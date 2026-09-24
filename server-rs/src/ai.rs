//! `/api/ai/*` 的第一批（P4 第 16 步）—— 起手是**只读的 provider 清单**。
//!
//! 为什么从它开始（总纲 §9 ③ 的顺序）：界面打开 AI 工作台第一件事就是拉它，
//! 而它**不含任何密钥**（AIH-012）—— 只有"这个 ref 配没配"这一个事实。
//!
//! # 两个最容易做错的地方（都会**静默**）
//!
//! 1. **键集合**：`AiProviderDto` 有 11 个字段，其中 `credential`（对象）与
//!    `createdAt` / `updatedAt` 是最容易漏的三个 —— Kotlin 的 `Json.kt` 里
//!    `encodeDefaults = true`，所以**它们一定会被序列化**。"少一个键 = 界面上少一块内容"
//!    是这个仓库栽过好几次的那类故障。
//! 2. **凭据状态的三分支**（`CredentialService.describe`）：
//!    ref 为空 → `(false, "none", false)`；环境变量命中（`<ref>` 或 `COMFYHUB_CRED_<ref>`）
//!    → `(值非空, "env", **false**)`；否则看托管凭据文件里有没有这个 ref
//!    → `(有, "managed" / 没有 "none", **true**)`。
//!    ⚠ `source = "env"` 时 `writable = false`（环境变量里的 key 不该由界面改，AIH-013）；
//!    `none` 时 `writable = true`（界面可以写一把新的）。
//!
//! 托管凭据文件是 `<storage>\ai\credentials.dpapi.json`，**值由 DPAPI 加密**。
//! 这里只读**键名**（`configured` / `source` 只需"有没有这个 ref"）——
//! **不解密，也不该解密**；写成"永远 false"则是**假装没配过**，那属于红线 R13。

use serde::Serialize;

use crate::routes::{ApiError, AppState};

/// 凭据状态（只有"有没有 / 从哪来 / 能不能改"，**没有值**）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatus {
    pub configured: bool,
    /// env / managed / none
    pub source: String,
    /// 前端是否可写；env 来源只读（AIH-013）
    pub writable: bool,
}

impl CredentialStatus {
    fn none_readonly() -> Self {
        Self { configured: false, source: "none".into(), writable: false }
    }
}

/// provider 清单里的一行（与 Kotlin `AiProviderDto` 逐字同形）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderDto {
    pub id: String,
    pub display_name: String,
    pub api: String,
    /// ⚠ 键名是 **`baseURL`**（Kotlin 的 `AiProviderDto.baseURL` 原样序列化）——
    /// `rename_all = "camelCase"` 会把它变成 `baseUrl`，**差一个字母界面就读不到**。
    /// 这条就是这么被自己的用例抓出来的（断言精确键集合就是为了它）。
    #[serde(rename = "baseURL")]
    pub base_url: String,
    /// Kotlin 是 `credentialRef: String? = null` → 为 null 时**键不出现**（`explicitNulls = false`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
    pub endpoint_trust: String,
    pub enabled: bool,
    pub revision: i64,
    /// 只写凭据的状态，**绝不包含值**（AIH-012）
    pub credential: CredentialStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// 环境变量名的前缀（`CredentialService.ENV_PREFIX`，**改了要同时改两边**）。
const ENV_PREFIX: &str = "COMFYHUB_CRED_";

/// 凭据状态的判定 —— **纯逻辑 + 注入两个查询**，这样三分支都能单测。
///
/// * `env_of(name)`：取环境变量（`None` = 没这个变量，`Some("")` = 有但是空的）；
/// * `stored_has(ref)`：托管凭据文件里有没有这个 ref（**只问键名，不解密**）。
pub fn describe_credential(
    reference: Option<&str>,
    env_of: impl Fn(&str) -> Option<String>,
    stored_has: impl Fn(&str) -> bool,
) -> CredentialStatus {
    let Some(reference) = reference.map(str::trim).filter(|r| !r.is_empty()) else {
        return CredentialStatus::none_readonly();
    };
    // 两个名字都要认：`<ref>` 与 `COMFYHUB_CRED_<ref>`
    let from_env = env_of(reference).or_else(|| env_of(&format!("{ENV_PREFIX}{reference}")));
    if let Some(value) = from_env {
        // ⚠ 环境变量那一档 `writable=false`：界面不该改一个由环境变量给的 key
        return CredentialStatus { configured: !value.is_empty(), source: "env".into(), writable: false };
    }
    let stored = stored_has(reference);
    CredentialStatus {
        configured: stored,
        source: if stored { "managed".into() } else { "none".into() },
        // 走到这里说明没有环境变量挡着：界面可以写（也为"清掉一把配错的 key"留着）
        writable: true,
    }
}

/// `GET /api/ai/providers`（不含密钥）。
pub fn list_providers(state: &AppState) -> Result<Vec<AiProviderDto>, ApiError> {
    let rows: Vec<AiProviderRow> = state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, display_name, api, base_url, credential_ref, endpoint_trust, enabled,
                    revision, created_at, updated_at
             FROM ai_providers ORDER BY display_name, id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(AiProviderRow {
                    id: row.get(0)?,
                    display_name: row.get(1)?,
                    api: row.get(2)?,
                    base_url: row.get(3)?,
                    credential_ref: row.get(4)?,
                    endpoint_trust: row.get(5)?,
                    enabled: row.get::<_, i64>(6)? != 0,
                    revision: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })?;

    let stored = read_credential_refs(state);
    Ok(rows
        .into_iter()
        .map(|row| {
            let credential = describe_credential(
                row.credential_ref.as_deref(),
                |name| std::env::var(name).ok(),
                |reference| stored.iter().any(|r| r == reference),
            );
            AiProviderDto {
                id: row.id,
                display_name: row.display_name,
                api: row.api,
                base_url: row.base_url,
                credential_ref: row.credential_ref,
                endpoint_trust: row.endpoint_trust,
                enabled: row.enabled,
                revision: row.revision,
                credential,
                created_at: row.created_at,
                updated_at: row.updated_at,
            }
        })
        .collect())
}

struct AiProviderRow {
    id: String,
    display_name: String,
    api: String,
    base_url: String,
    credential_ref: Option<String>,
    endpoint_trust: String,
    enabled: bool,
    revision: i64,
    created_at: Option<String>,
    updated_at: Option<String>,
}

/// 托管凭据文件里的**键名**（`<storage>\ai\credentials.dpapi.json`）。
///
/// 只读键名：值是用 DPAPI 加密的，**不归我们解**（也不该解）。
/// 文件不存在 / 读坏了一律当"没有"—— 那正是"没配过"的意思，不能因此让整个端点 500。
fn read_credential_refs(state: &AppState) -> Vec<String> {
    let file = credential_store_path(state);
    let Ok(text) = std::fs::read_to_string(&file) else { return Vec::new() };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    map.keys().cloned().collect()
}

/// `<storage>\ai\credentials.dpapi.json`（`media_dir = <storage>\media`）。
fn credential_store_path(state: &AppState) -> std::path::PathBuf {
    let storage = state.media_dir().parent().unwrap_or(state.media_dir());
    storage.join("ai").join("credentials.dpapi.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn 凭据状态的三个分支一条都不能错() {
        // ① ref 为空：不配置、不可写（连"能不能写"都是 false：没地方写）
        let s = describe_credential(None, no_env, |_| true);
        assert_eq!(s, CredentialStatus { configured: false, source: "none".into(), writable: false });
        let s = describe_credential(Some("   "), no_env, |_| true);
        assert_eq!(s.source, "none", "空白 ref 与没有 ref 同义");

        // ② 环境变量命中 → source=env 且 **writable=false**（AIH-013：界面不许改环境变量给的 key）
        let s = describe_credential(Some("OPENAI_KEY"), |n| (n == "OPENAI_KEY").then(|| "sk-x".into()), |_| false);
        assert_eq!(s, CredentialStatus { configured: true, source: "env".into(), writable: false });
        // 空串的环境变量 = 配了但没值
        let s = describe_credential(Some("OPENAI_KEY"), |_| Some(String::new()), |_| false);
        assert_eq!(s, CredentialStatus { configured: false, source: "env".into(), writable: false });

        // ③ 前缀那个名字也要认（`COMFYHUB_CRED_<ref>`）——认错了就等于"永远读不到那把 key"
        let s = describe_credential(Some("OPENAI_KEY"), |n| (n == "COMFYHUB_CRED_OPENAI_KEY").then(|| "sk-y".into()), |_| false);
        assert_eq!(s.source, "env");
        assert!(s.configured);

        // ④ 托管文件里有没有 → managed / none；**两种都可写**（界面要能改、也要能清一把配错的）
        let s = describe_credential(Some("OPENAI_KEY"), no_env, |_| true);
        assert_eq!(s, CredentialStatus { configured: true, source: "managed".into(), writable: true });
        let s = describe_credential(Some("OPENAI_KEY"), no_env, |_| false);
        assert_eq!(s, CredentialStatus { configured: false, source: "none".into(), writable: true });

        // ⑤ 环境变量优先于托管文件（与 Kotlin 的判定顺序一致）
        let s = describe_credential(Some("K"), |_| Some("v".into()), |_| true);
        assert_eq!(s.source, "env", "环境变量先判");
    }
}
