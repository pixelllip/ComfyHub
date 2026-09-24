//! 同步 API（P4 第 10 步）：`GET /api/sync/manifest`、`GET /api/sync/changes`。
//!
//! 契约见 🔒 `docs/sync-api-contract.md` §1。这是手机连上来的**第一件事**：
//! "我该不该走 bootstrap？我的游标还有效吗？"
//!
//! # 同步范围用显式白名单（而不是"查一下有哪些触发器"）
//!
//! 先说清**今天的事实**（2026-09-24 核对过 `db/schema.sqlite.sql`）：真正挂 `_sync_log`
//! 触发器的表**正好就是** [SYNCED] 这 10 张，一张不多一张不少。所以"从触发器派生范围"
//! 在今天**不会**出错 —— 别把它当成"现在就有洞"。
//!
//! 但仍然坚持写死白名单，因为这条路径的**失败模式是静默且严重的**：
//!
//! - `app_settings` 里存着**配对令牌明文**、`ai_providers` 里是凭据引用。
//!   哪天有人给这两张表补一个 `_sync_log` 触发器（例如"设置也想同步到手机"），
//!   派生式的实现会**立刻开始把令牌发给手机**，而且不会有任何测试变红。
//! - 契约把"总纲 §3.3 的那张表"指定为**唯一真源**（触发器 / `bootstrap` / `changes`
//!   三处都从它派生）。白名单就是那张表在代码里的落地 —— 让三处派生自同一个常量，
//!   而不是各自去问 schema。
//! - 注意区分**两类触发器**：`trg_<表>_sync_*` 才是账本触发器，`trg_<表>_touch` 只是
//!   维护 `updated_at` 的（`app_settings` / `ai_providers` / `ai_models` / `ai_runs`
//!   只有后者）。当初就是差点把 `touch` 当成 `sync` 才误判了一次 ——
//!   防线是 [`tests::synced_scope_matches_the_master_plan`] 与
//!   `api_test.rs` 里那条"注入一条假的 `app_settings` 账本行，看它会不会被发出去"。

use std::collections::BTreeMap;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

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

// ---------------------------------------------------------------------------
//  GET /api/sync/changes
// ---------------------------------------------------------------------------

/// 主键的形态 —— 决定 `deletes` 里那个键怎么表示（契约 §2「主键表示」）。
#[derive(Debug, Clone, Copy, PartialEq)]
enum PkKind {
    /// 单列整数主键（`prompts` / `tags` / `media_assets` / `capture_runs`）→ 标量数字
    Int,
    /// 单列文本主键（会话 / 消息 / 块 / 附件都是 UUID）→ 字符串
    Text,
    /// 复合主键（`prompt_tags` / `media_tags`），账本里是 `a:b` → 数组 `[a, b]`
    Composite(&'static str, &'static str),
}

/// 每张同步表的**主键列**。表名与列名都是编译期常量 → 下面拼 SQL 不是注入面。
fn pk_kind(table: &str) -> Option<PkKind> {
    Some(match table {
        "prompts" | "tags" | "media_assets" | "capture_runs" => PkKind::Int,
        "ai_conversations" | "ai_messages" | "ai_message_parts" | "ai_attachments" => PkKind::Text,
        "prompt_tags" => PkKind::Composite("prompt_id", "tag_id"),
        "media_tags" => PkKind::Composite("media_id", "tag_id"),
        _ => return None,
    })
}

/// 单列主键表的取行/取键列（复合表由 [PkKind::Composite] 自己带）。
fn single_pk_column(table: &str) -> &'static str {
    match table {
        "prompts" | "tags" | "media_assets" | "capture_runs" => "id",
        "ai_conversations" => "id",
        "ai_messages" => "id",
        "ai_message_parts" => "id",
        "ai_attachments" => "id",
        _ => "id",
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncChanges {
    /// 本次扫描到的最大 `seq`（**唯独不能跳号**）；没有新变更时 = 当前 `MAX(_sync_log.seq)`。
    pub cursor: i64,
    /// true 表示还有下一页，**手机不能只看第一页就更新游标**。
    pub has_more: bool,
    /// `{表: [完整行]}`（不是 diff：手机直接覆盖本地行）。
    pub upserts: BTreeMap<String, Vec<serde_json::Value>>,
    /// `{表: [主键]}` —— **必须独立成组**：手机分不清"我没这行"与"这行被删了"。
    pub deletes: BTreeMap<String, Vec<serde_json::Value>>,
}

/// 一次账本扫描的原始条目。
struct LogRow {
    seq: i64,
    tbl: String,
    pk: String,
    op: String,
}

/// `GET /api/sync/changes?since=<cursor>&limit=500`
///
/// # 两个不能省的分支
///
/// 1. **游标落在保留窗口之外 → `410 Gone`**。裁剪与 410 是**一对**（契约 §5）：
///    正因为老 `seq` 被裁掉了才需要这个分支。缺了它的表现是"手机少了几百条，
///    而且**永远补不回来**" —— 因为增量接口照着它给的游标往后扫，永远扫不到那些洞。
/// 2. **`deletes` 独立成组**。混进 `upserts` 就变成"客户端永远留着幽灵行"。
pub fn changes(state: &AppState, since: i64, limit: i64) -> Result<SyncChanges, ApiError> {
    let limit = limit.clamp(1, 500);
    let since = since.max(0);
    // `AppState::with` 的闭包只能回 `rusqlite::Result`（见它的签名），所以"要不要 410"
    // 先当成一个**结果枚举**带出来，出了连接锁再翻成 ApiError。
    match state.with(|conn| changes_on(conn, since, limit))? {
        ChangesOutcome::Gone { since, min } => Err(ApiError::gone(format!(
            "游标 {since} 早于保留窗口（最老 seq = {min}）：请丢弃镜像改走 /api/sync/bootstrap"
        ))),
        ChangesOutcome::Changes(c) => Ok(c),
    }
}

enum ChangesOutcome {
    /// 游标落在保留窗口之外 → 调用方翻成 `410 Gone`
    Gone { since: i64, min: i64 },
    Changes(SyncChanges),
}

/// [changes] 的实现体（单独一个函数：`state.with` 的闭包没法标返回类型）。
fn changes_on(conn: &rusqlite::Connection, since: i64, limit: i64) -> rusqlite::Result<ChangesOutcome> {
    {
        // 保留窗口的左边界。空账本 = 无窗口（不 410：新手机本来就没游标）
        let min_seq: Option<i64> = conn.query_row("SELECT MIN(seq) FROM _sync_log", [], |row| {
            row.get::<_, Option<i64>>(0)
        })?;
        // `since == min_seq - 1` 是安全的：从 since 之后一个不落。再小就有洞了
        if let Some(min) = min_seq {
            if since > 0 && since < min - 1 {
                return Ok(ChangesOutcome::Gone { since, min });
            }
        }

        // 只扫**同步范围内**的表：触发器覆盖了 app_settings 等表，那些条目就在这里被挡掉
        //
        // ⚠ 占位符**一律用无名 `?`**（按出现顺序从 1 编号）。混用 `?1` 与 `?` 会出事：
        // `?` 拿的是"下一个未用的号"，于是 `IN (?)` 会把 `?2` 占掉，后面的 `LIMIT ?2`
        // 就与它撞号 —— 表现是**整个端点 500**（而不是某个字段不对）。
        let placeholders = vec!["?"; SYNCED.len()].join(",");
        let sql = format!(
            "SELECT seq, tbl, pk, op FROM _sync_log
             WHERE seq > ? AND tbl IN ({placeholders})
             ORDER BY seq ASC LIMIT ?"
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(SYNCED.len() + 2);
        params.push(Box::new(since));
        for name in SYNCED {
            params.push(Box::new((*name).to_string()));
        }
        // 多取一条用来判断 hasMore（不要先 COUNT 一遍再扫一遍：白扫一次）
        params.push(Box::new(limit + 1));
        let mut stmt = conn.prepare(&sql)?;
        let rows: Vec<LogRow> = stmt
            .query_map(rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())), |row| {
                Ok(LogRow { seq: row.get(0)?, tbl: row.get(1)?, pk: row.get(2)?, op: row.get(3)? })
            })?
            .collect::<Result<_, _>>()?;

        let has_more = rows.len() as i64 > limit;
        let window: Vec<&LogRow> = rows.iter().take(limit as usize).collect();

        let mut upserts: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
        let mut deletes: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();

        for row in &window {
            let Some(kind) = pk_kind(&row.tbl) else { continue };
            if row.op == "delete" {
                if let Some(key) = delete_key(kind, &row.pk) {
                    deletes.entry(row.tbl.clone()).or_default().push(key);
                }
                continue;
            }
            // upsert：取**完整行**。行已经不在了就跳过（说明后面还有一条 delete 条目
            // 会被送到 —— 由它负责让手机删掉本地那行）
            if let Some(full) = fetch_row(conn, &row.tbl, kind, &row.pk)? {
                upserts.entry(row.tbl.clone()).or_default().push(full);
            }
        }

        // 游标：这一页没扫完 → 停在扫到的最后一条；扫完了 → 可以跳到账本末尾
        // （跨过那些**不属于同步范围**的条目，否则手机永远推不过去）
        let cursor = if has_more {
            window.last().map(|r| r.seq).unwrap_or(since)
        } else {
            let max: i64 =
                conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM _sync_log", [], |row| row.get(0))?;
            max.max(since)
        };

        Ok(ChangesOutcome::Changes(SyncChanges { cursor, has_more, upserts, deletes }))
    }
}

/// `deletes` 里的主键：单列 → 标量；复合 → 数组（契约 §2）。
fn delete_key(kind: PkKind, pk: &str) -> Option<serde_json::Value> {
    match kind {
        PkKind::Int => pk.parse::<i64>().ok().map(|n| serde_json::json!(n)),
        PkKind::Text => Some(serde_json::json!(pk)),
        PkKind::Composite(_, _) => {
            let mut parts = pk.splitn(2, ':');
            let a = parts.next()?.parse::<i64>().ok()?;
            let b = parts.next()?.parse::<i64>().ok()?;
            Some(serde_json::json!([a, b]))
        }
    }
}

/// 按主键取一整行，泛型映射成 JSON（**不维护每张表的列清单**：
/// 列清单一旦漂移就会"少发一列"，而界面上少一列是**静默**的）。
fn fetch_row(
    conn: &rusqlite::Connection,
    table: &str,
    kind: PkKind,
    pk: &str,
) -> rusqlite::Result<Option<serde_json::Value>> {
    let sql = match kind {
        PkKind::Composite(a, b) => format!(
            "SELECT * FROM {table} WHERE CAST({a} AS TEXT) || ':' || CAST({b} AS TEXT) = ?1"
        ),
        _ => format!("SELECT * FROM {table} WHERE CAST({} AS TEXT) = ?1", single_pk_column(table)),
    };
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([pk])?;
    match rows.next()? {
        None => Ok(None),
        Some(row) => Ok(Some(row_to_json(row)?)),
    }
}

/// 一行 → JSON 对象（列名 → 值）。
fn row_to_json(row: &rusqlite::Row) -> rusqlite::Result<serde_json::Value> {
    use rusqlite::types::ValueRef;
    let mut map = serde_json::Map::new();
    for (i, name) in row.as_ref().column_names().iter().enumerate() {
        let value = match row.get_ref(i)? {
            ValueRef::Null => serde_json::Value::Null,
            ValueRef::Integer(n) => serde_json::json!(n),
            ValueRef::Real(f) => serde_json::json!(f),
            ValueRef::Text(t) => serde_json::json!(String::from_utf8_lossy(t).into_owned()),
            // 契约 §1：二进制不进库（走 HTTP + ETag）。真出现 blob 说明 schema 被破了 ——
            // 用十六进制如实带出去，**不许静默变成 null**（那就是悄悄丢数据）
            ValueRef::Blob(b) => serde_json::json!(b.iter().map(|x| format!("{x:02x}")).collect::<String>()),
        };
        map.insert(name.to_string(), value);
    }
    Ok(serde_json::Value::Object(map))
}

// ---------------------------------------------------------------------------
//  POST /api/sync/push
// ---------------------------------------------------------------------------

/// 手机 `kind` → 桌面表名（**单数 kind、复数表**，与契约 §4 的例子同形）。
fn table_of_kind(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "ai_conversation" => "ai_conversations",
        "ai_message" => "ai_messages",
        "ai_message_part" => "ai_message_parts",
        "ai_attachment" => "ai_attachments",
        _ => return None,
    })
}

/// **手机不许创建的表**（总纲 §3.3：那些由桌面在提交时产生）。
///
/// 推上来只能是 `rejected`，而且要给出**永久性**的错误码 —— 手机据此把 outbox 那行丢掉
/// （若回一个"暂时失败"，手机就会永远重试，每次都被拒）。
const PHONE_FORBIDDEN: &[&str] = &[
    "prompts",
    "media_assets",
    "tags",
    "prompt_tags",
    "media_tags",
    "capture_runs",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushRequest {
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub batch: Vec<PushItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushItem {
    pub kind: String,
    pub ref_id: String,
    #[serde(default)]
    pub op: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushResult {
    pub applied: Vec<String>,
    pub rejected: Vec<Rejected>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rejected {
    pub ref_id: String,
    /// **逐条给码**（契约 §4）：手机要能区分"永远不可能成功"（丢掉）与"暂时失败"（重试）
    pub code: String,
    pub message: String,
}

/// `POST /api/sync/push` —— 上行：把手机 `_outbox` 里攒的东西推上来（契约 §4）。
///
/// # 三条纪律
///
/// 1. **幂等**：重复推送**不许**产生重复行。靠"主键是 UUID + `ON CONFLICT DO UPDATE`"。
///    第二次推同一条会落到 LWW 分支被记 `STALE_IGNORED`（不是插入第二行）。
/// 2. **服务端赢（LWW）**：只有 `updated_at` **比桌面新**才覆盖；否则丢弃并记 `rejected`。
///    桌面是唯一真源（架构纪律 1），不能让手机把桌面改过的行回退成旧值。
/// 3. **`rejected` 逐条给 `code`**，而且**永久性失败与暂时失败要分清** ——
///    给一个"暂时"的码会让手机永远重试一条永远不会成功的行。
///
/// 每条 item **各自一个事务**：坏行不该挡住同批里其它行，手机也才能逐条清 outbox。
pub fn push(state: &AppState, request: PushRequest) -> Result<PushResult, ApiError> {
    if request.batch.len() > 500 {
        return Err(ApiError::bad_request(format!(
            "单批最多 500 行，收到 {}",
            request.batch.len()
        )));
    }
    let mut result = PushResult { applied: Vec::new(), rejected: Vec::new() };
    for item in &request.batch {
        match apply_one(state, item) {
            Ok(()) => result.applied.push(item.ref_id.clone()),
            Err(reject) => result.rejected.push(reject),
        }
    }
    Ok(result)
}

/// 应用一条上行：**每条一个事务**（`BEGIN IMMEDIATE`，R2）。
fn apply_one(state: &AppState, item: &PushItem) -> Result<(), Rejected> {
    let reject = |code: &str, message: String| Rejected {
        ref_id: item.ref_id.clone(),
        code: code.to_string(),
        message,
    };

    // ① 允许的表（先看是不是"手机根本不配创建的"
    if PHONE_FORBIDDEN.contains(&item.kind.as_str()) {
        return Err(reject(
            "FORBIDDEN_KIND",
            format!("{} 由桌面在提交时产生，手机不许创建（总纲 §3.3）", item.kind),
        ));
    }
    let Some(table) = table_of_kind(&item.kind) else {
        return Err(reject("UNKNOWN_KIND", format!("未知的上行类型：{}", item.kind)));
    };
    // ② op：目前只支持 upsert（delete 是"手机删桌面数据"，还没定策略）
    if let Some(op) = item.op.as_deref() {
        if op != "upsert" {
            return Err(reject("UNSUPPORTED_OP", format!("暂不支持的操作：{op}")));
        }
    }
    let Some(obj) = item.payload.as_object() else {
        return Err(reject("BAD_PAYLOAD", "payload 必须是对象（完整行）".to_string()));
    };

    state
        .write(|tx| {
            // ③ 列清单**从 PRAGMA 现查**（不维护手写清单：漂移了就会静默少写一列）。
            //    列名验证过字符集之后再拼进 SQL —— payload 的键来自手机，这里是注入面。
            let columns = table_columns(tx, table)?;
            if columns.is_empty() {
                return Ok(Err(reject("UNKNOWN_TABLE", format!("桌面没有 {table} 这张表"))));
            }
            if !columns.iter().any(|c| c == "id") {
                return Ok(Err(reject("SCHEMA_MISMATCH", format!("{table} 没有 id 列"))));
            }

            let mut cols: Vec<String> = Vec::new();
            let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            for (key, value) in obj {
                if !columns.iter().any(|c| c == key) {
                    // 未知列 = 两端 schema 对不上，**永久性失败**（别让手机死循环重试）
                    return Ok(Err(reject(
                        "SCHEMA_MISMATCH",
                        format!("{table} 没有列 {key}"),
                    )));
                }
                cols.push(key.clone());
                values.push(json_to_sql(value));
            }
            if !cols.iter().any(|c| c == "id") {
                return Ok(Err(reject("BAD_PAYLOAD", "payload 缺 id".to_string())));
            }
            // ref_id 与 payload.id 必须一致，否则 outbox 的引用就是错的
            if obj.get("id").and_then(|v| v.as_str()) != Some(item.ref_id.as_str()) {
                return Ok(Err(reject(
                    "REF_MISMATCH",
                    format!("refId ({}) 与 payload.id 不一致", item.ref_id),
                )));
            }

            // ④ LWW：只有比桌面新才覆盖
            if let Some(incoming) = obj.get("updated_at").and_then(|v| v.as_str()) {
                let existing: Option<String> = tx
                    .query_row(
                        &format!("SELECT updated_at FROM {table} WHERE id = ?1"),
                        [item.ref_id.as_str()],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten();
                if let Some(current) = existing {
                    if incoming <= current.as_str() {
                        return Ok(Err(reject(
                            "STALE_IGNORED",
                            format!("桌面的那一行不更旧（{current} >= {incoming}），服务端赢"),
                        )));
                    }
                }
            }

            // ⑤ 幂等 upsert：**绝不能用 INSERT OR REPLACE**
            //    （R3：它内部是 DELETE+INSERT，会点亮 DELETE 触发器往 _sync_log 写**假的 delete**）
            let quoted = cols.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(", ");
            let holders = vec!["?"; cols.len()].join(", ");
            let updates = cols
                .iter()
                .filter(|c| c.as_str() != "id")
                .map(|c| format!("{0} = excluded.{0}", quote_ident(c)))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "INSERT INTO {table} ({quoted}) VALUES ({holders})
                 ON CONFLICT(id) DO UPDATE SET {updates}"
            );
            let refs: Vec<&dyn rusqlite::ToSql> = values.iter().map(|v| v.as_ref()).collect();
            tx.execute(&sql, rusqlite::params_from_iter(refs))?;
            Ok(Ok(()))
        })
        .map_err(|e: ApiError| reject("DB_ERROR", format!("落库失败：{}", e.message)))?
}

/// 表有哪些列（`PRAGMA table_info`，与 P2 的收敛式迁移同一个取数方式）。
fn table_columns(tx: &rusqlite::Transaction<'_>, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = tx.prepare(&format!("PRAGMA table_info({})", quote_ident(table)))?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(names)
}

/// 列名只允许 `[A-Za-z0-9_]`，然后加双引号 —— 双保险（键来自手机，是注入面）。
fn quote_ident(name: &str) -> String {
    let safe: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    format!("\"{safe}\"")
}

/// JSON 值 → SQLite 参数。对象/数组按 JSON 文本存（schema 里 JSON 列就是 TEXT）。
fn json_to_sql(value: &serde_json::Value) -> Box<dyn rusqlite::ToSql> {
    match value {
        serde_json::Value::Null => Box::new(Option::<String>::None),
        serde_json::Value::Bool(b) => Box::new(if *b { 1i64 } else { 0i64 }),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Box::new(i)
            } else {
                Box::new(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Box::new(s.clone()),
        other => Box::new(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **没有任何一张"带凭据/令牌"的表能出现在 `synced: true` 里。**
    ///
    /// 这条用例盯着白名单本身：谁把 `app_settings` 加进 [SYNCED]（哪怕理由是
    /// "设置也该同步到手机"），这里立刻红。
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
