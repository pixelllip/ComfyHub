//! 同步 API（P4 第 10 步）：`GET /api/sync/manifest`。
//!
//! 契约见 🔒 `docs/sync-api-contract.md` §1。这是手机连上来的**第一件事**：
//! "我该不该走 bootstrap？我的游标还有效吗？"
//!
//! # ⚠ 同步范围**必须**用显式白名单，绝不能从触发器派生
//!
//! 直觉上"被同步的表 = 挂了 `_sync_log` 触发器的表"很诱人，但在这份 schema 上
//! **那是错的，而且错得有安全后果**：实际建出来的触发器覆盖了 `app_settings`、
//! `ai_providers`、`ai_models`（见 `db/schema.sqlite.sql`），而总纲 §3.3 的同步范围
//! **明确排除**它们 —— `app_settings` 里存着**配对令牌明文**，`ai_providers` 里存着凭据引用。
//! 一旦按触发器派生 `synced`，手机第一次连上来就会看到 `app_settings` 是"可同步的表"，
//! 接下来的 `bootstrap` / `changes` 就会把这些行发出去 —— 这正是契约 §3 那句
//! "绝不发 .db 文件：`app_settings` 里有配对令牌明文"要防的事。
//!
//! 所以这里把范围**写死成两张表**（[SYNCED] / [NOT_SYNCED]），并以"总纲 §3.3 是唯一真源"
//! 为准；`SYNCED` 里每一张都必须真的存在（不存在=有人改了 schema 没改这里，值得当场发现）。
//! 防线是 `manifest_never_offers_credentials_or_settings` 那条用例。

use std::collections::BTreeMap;

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::routes::{ApiError, AppState};

/// **同步**的表（总纲 §3.3「同步」列的逐字落地）。
pub const SYNCED: &[&str] = &[
    "prompts",
    "tags",
    "prompt_tags",
    "media_assets",
    "media_tags",
    "ai_conversations",
    "ai_messages",
    "ai_message_parts",
    "ai_attachments",
    "capture_runs",
];

/// **不同步**的表 —— 每一条都要有理由（`reason` 会原样吐给手机做诊断）。
///
/// 前四条是**安全相关**的，别顺手加进 [SYNCED]：
/// `app_settings`（配对令牌明文）、`ai_providers`（凭据引用）、`ai_models`（模型目录由
/// run-kit 下发）、`ai_run_events`（62 万行调试日志）。
pub const NOT_SYNCED: &[(&str, &str)] = &[
    ("app_settings", "含配对令牌明文与本地设置，绝不外发"),
    ("ai_providers", "含凭据引用，只在桌面侧"),
    ("ai_models", "模型目录由 run-kit 版本化下发"),
    ("ai_run_events", "调试日志，按设计不同步"),
    ("ai_runs", "工具过程与事件流（总纲 §7 待定项 1，当前不同步）"),
    ("ai_tool_calls", "审批与工具调用明细，当前只留桌面"),
    ("ai_questions", "提问闸门只在桌面侧，当前不同步"),
];

/// `app_settings` 里 run-kit 版本号的键（与 Kotlin `RunKit.KEY_VERSION` 同值）。
const KEY_KIT_VERSION: &str = "ai.runkit.version";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncManifest {
    pub schema_version: i64,
    /// 桌面已知的 run-kit 版本（Kotlin 侧写过才有；没写过就是 null，**不编一个**）。
    pub kit_version: Option<String>,
    /// `MAX(_sync_log.seq)`；空账本为 0（手机据此判断"桌面还没有任何变更"）。
    pub cursor: i64,
    pub server_time: String,
    pub tables: BTreeMap<String, TableStatus>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TableStatus {
    pub synced: bool,
    /// 行数**只对同步表给**（不同步的表连行数都不该外发 —— 行数本身也是信息）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `GET /api/sync/manifest`
pub fn manifest(state: &AppState) -> Result<SyncManifest, ApiError> {
    state.with(|conn| {
        let schema_version = crate::db::schema_version(conn)?;
        let cursor: i64 = conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM _sync_log", [], |row| row.get(0))?;
        // 时间与库里其余地方同口径（TEXT ISO-8601 UTC），不引 chrono
        let server_time: String =
            conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')", [], |row| row.get(0))?;
        let kit_version: Option<String> = conn
            .query_row("SELECT v FROM app_settings WHERE k = ?1", [KEY_KIT_VERSION], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten()
            .filter(|v| !v.trim().is_empty());

        let mut tables = BTreeMap::new();
        for name in SYNCED {
            // 白名单写死，但**表是不是真的在**要问库：不在说明 schema 与这里脱节了
            let exists: Option<String> = conn
                .query_row("SELECT name FROM sqlite_master WHERE type='table' AND name = ?1", [name], |row| {
                    row.get(0)
                })
                .optional()?;
            if exists.is_none() {
                continue;
            }
            // 表名来自**编译期常量白名单**（不是用户输入），所以 format! 不是注入面
            let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {name}"), [], |row| row.get(0))?;
            tables.insert(
                (*name).to_string(),
                TableStatus { synced: true, row_count: Some(count), reason: None },
            );
        }
        for (name, reason) in NOT_SYNCED {
            let exists: Option<String> = conn
                .query_row("SELECT name FROM sqlite_master WHERE type='table' AND name = ?1", [name], |row| {
                    row.get(0)
                })
                .optional()?;
            if exists.is_none() {
                continue;
            }
            tables.insert(
                (*name).to_string(),
                TableStatus { synced: false, row_count: None, reason: Some((*reason).to_string()) },
            );
        }

        Ok(SyncManifest { schema_version, kit_version, cursor, server_time, tables })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **没有任何一张"带凭据/令牌"的表能出现在 `synced: true` 里。**
    ///
    /// 这条用例就是那份白名单存在的理由：触发器覆盖了 `app_settings`，
    /// 谁要是图省事改成"从触发器派生"，这条会立刻红。
    #[test]
    fn credentials_and_settings_are_never_synced() {
        for dangerous in ["app_settings", "ai_providers", "ai_models", "ai_run_events"] {
            assert!(!SYNCED.contains(&dangerous), "{dangerous} 绝不能被同步");
            let entry = NOT_SYNCED.iter().find(|(n, _)| *n == dangerous);
            assert!(entry.is_some(), "{dangerous} 必须显式列出并给出理由（不能只是'没提到'）");
            assert!(!entry.unwrap().1.trim().is_empty(), "{dangerous} 的理由不能是空的");
        }
    }

    /// 同步范围与总纲 §3.3 逐字一致（少一张 = 手机永远收不到那种数据）。
    #[test]
    fn synced_scope_matches_the_master_plan() {
        let expected = [
            "prompts",
            "tags",
            "prompt_tags",
            "media_assets",
            "media_tags",
            "ai_conversations",
            "ai_messages",
            "ai_message_parts",
            "ai_attachments",
            "capture_runs",
        ];
        assert_eq!(SYNCED, &expected, "改同步范围要同时改总纲 §3.3 与契约");
        // 两张表不能同时出现在两个清单里
        for name in SYNCED {
            assert!(
                !NOT_SYNCED.iter().any(|(n, _)| n == name),
                "{name} 同时出现在同步与不同步清单里",
            );
        }
    }

    #[test]
    fn kit_version_key_matches_the_kotlin_side() {
        // 键名写错不会报错，只会让 kitVersion 永远为 null（手机以为规则工件过期）
        assert_eq!(KEY_KIT_VERSION, "ai.runkit.version");
    }
}
