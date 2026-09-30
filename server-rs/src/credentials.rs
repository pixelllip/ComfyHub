//! 托管凭据的**写侧**（P4 第 34 步）—— `CredentialService` 的 `set` / `unset` / `sanitize`。
//!
//! 读侧（`describe`，只有"配没配"）在 P4 第 16 步就搬了；这里补上"**怎么把一把 key 存进去**"，
//! 于是 `/api/ai/providers/{id}/credentials` 与 `/api/ai/web-search/credential` 这两组
//! 界面端点才有意义（否则设置页那两张卡读得到状态、写不进去）。
//!
//! # 文件与两端共用
//!
//! `<storage>\ai\credentials.dpapi.json` 是**一份**文件，Kotlin 与 Rust 两个实现都可能写它
//! （灰度期随机指向哪一个后端）。所以三件事必须逐字对齐 Kotlin `CredentialService`：
//!
//! | 事 | Kotlin | 这里 |
//! | --- | --- | --- |
//! | 值的编码 | `ProtectedData.Protect(空熵, CurrentUser)` → `Base64.getEncoder()` | `CryptProtectData(空熵, UI_FORBIDDEN)` → [`crate::auth::base64_std`]（**标准字母表 + 填充**） |
//! | 写入方式 | 先写 `*.tmp` 再 `Files.move(REPLACE_EXISTING)` | 先写 `*.tmp` 再 `rename`（**同一个目录**，Windows 上 rename 才覆盖得掉） |
//! | 读坏文件 | `runCatching{…}.getOrElse { emptyMap() }` | 同样当"没有"（**不 500**） |
//!
//! # 三条不许退让的纪律
//!
//! 1. **DPAPI 不可用就直接失败**（AIH-015）：绝不退化成明文落盘 ——
//!    把明文包装成"加密存储"比明文存文件更坏（用户以为它是加密的）。
//! 2. **值可以空 = 不修改**（AIH-012/013）：`PUT` 传空串不是"清空"，清空只能走 `DELETE`。
//!    这条在路由层判（`body.value.trim().is_empty()` → 直接回状态、不写库）。
//! 3. **env 来源只读**（AIH-013）：环境变量给的 key 不许在界面里改/删 ——
//!    写进去也读不出来（`resolve` 里环境变量优先），只会让界面显示"已配置"而实际不生效。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ai::ENV_PREFIX;
use crate::routes::{ApiError, AppState};

/// 联网搜索（Keenable）的凭据 ref（`KeenableSearch.CREDENTIAL_REF`，**改了要同时改两端**）。
pub const KEENABLE_REF: &str = "keenable";

/// 托管凭据库的路径（`<storage>\ai\credentials.dpapi.json`）——与读侧**同一个函数**。
pub fn store_path(state: &AppState) -> PathBuf {
    crate::ai::credential_store_path(state)
}

/// 输入校验（`CredentialService.sanitize` 的逐条对应物）。
///
/// 这三条报错都是**为了不让一把错的 key 悄悄存进去**：粘贴整行 `NAME=value`、
/// 带引号、带空白 —— 它们在界面上看起来都"填好了"，直到上游回 401 才发现。
pub fn sanitize(raw: &str) -> Result<String, ApiError> {
    // Kotlin `isBlank()`（含 Unicode 空白）；这里用 `trim().is_empty()` 同口径
    if raw.trim().is_empty() {
        return Err(ApiError::ai("CONFIG_ERROR", "密钥不能为空；留空表示不修改已有密钥"));
    }
    let value = raw.trim().to_string();

    let first = value.chars().next();
    let last = value.chars().last();
    if (first == Some('"') && last == Some('"')) || (first == Some('\'') && last == Some('\'')) {
        return Err(ApiError::ai("CONFIG_ERROR", "请只粘贴密钥本身，不要带引号"));
    }
    if let Some(eq) = value.find('=') {
        // `substringBefore('=')` 是**第一个** `=` 之前那一段
        if is_env_name(&value[..eq]) {
            return Err(ApiError::ai(
                "CONFIG_ERROR",
                "请只粘贴密钥的值，不要粘贴 NAME=value 这种整行",
            ));
        }
    }
    // `^[\x21-\x7E]+$`：可打印 ASCII，且不许有空格/换行（值要进 HTTP Header）
    if !value.chars().all(|c| ('\u{21}'..='\u{7E}').contains(&c)) {
        return Err(ApiError::ai("CONFIG_ERROR", "密钥只能包含可打印字符，且不能有空格或换行"));
    }
    Ok(value)
}

/// `^[A-Za-z_][A-Za-z0-9_]*$`（Java 那条正则的对应物）。
fn is_env_name(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 读整个库：`ref → base64(密文)`。**不解密**（读侧从来不需要明文）。
///
/// 文件不存在 / 不是 JSON / 不是对象 → 一律当空（与 Kotlin 的 `getOrElse { emptyMap() }` 同）。
/// 非字符串的值按 Kotlin `JsonPrimitive.content` 的语义取字面量（数字/布尔），
/// 对象与数组**跳过**（`(v as? JsonPrimitive)` 在那边是 null）。
pub fn read_store(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&text) else {
        return BTreeMap::new();
    };
    map.into_iter()
        .filter_map(|(key, value)| match value {
            serde_json::Value::String(s) => Some((key, s)),
            serde_json::Value::Number(n) => Some((key, n.to_string())),
            serde_json::Value::Bool(b) => Some((key, b.to_string())),
            _ => None,
        })
        .collect()
}

/// 写整个库：先写 `*.tmp`、**收紧权限**、再 `rename` 覆盖（同目录，Windows 上也原子）。
pub fn write_store(path: &Path, store: &BTreeMap<String, String>) -> Result<(), ApiError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| ApiError::ai("CONFIG_ERROR", format!("无法创建凭据目录（{}）：{e}", dir.display())))?;
    }
    let mut map = serde_json::Map::with_capacity(store.len());
    for (key, value) in store {
        map.insert(key.clone(), serde_json::Value::String(value.clone()));
    }
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(map))
        .map_err(|e| ApiError::internal(format!("凭据库序列化失败：{e}")))?;

    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    std::fs::write(&tmp, text)
        .map_err(|e| ApiError::ai("CONFIG_ERROR", format!("无法写入凭据文件（{}）：{e}", tmp.display())))?;
    restrict_permissions(&tmp);
    std::fs::rename(&tmp, path)
        .map_err(|e| ApiError::ai("CONFIG_ERROR", format!("无法替换凭据文件（{}）：{e}", path.display())))?;
    Ok(())
}

/// 双保险：DPAPI 之外再收紧属主权限（Windows 上 POSIX 视图拿不到，**静默跳过** ——
/// Kotlin 的 `restrictPermissions` 也是 `runCatching` 包着的）。
fn restrict_permissions(_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o600));
    }
}

/// 当前状态（`describe` 的现场版：环境变量 + 这个文件里的键名）。
pub fn describe_now(state: &AppState, reference: &str) -> crate::ai::CredentialStatus {
    let stored = read_store(&store_path(state));
    crate::ai::describe_credential(
        Some(reference),
        |name| std::env::var(name).ok(),
        |key| stored.contains_key(key),
    )
}

/// 这把 key 是不是由**环境变量**提供的（两个名字都认）——env 来源一切写入都拒绝。
pub fn env_source(reference: &str) -> Option<String> {
    std::env::var(reference)
        .ok()
        .or_else(|| std::env::var(format!("{ENV_PREFIX}{reference}")).ok())
}

/// 存一把 key（`set`）：校验 → DPAPI 加密 → 合并进库 → 原子落盘。
pub fn set(state: &AppState, reference: &str, raw_value: &str) -> Result<crate::ai::CredentialStatus, ApiError> {
    let value = sanitize(raw_value)?;
    let sealed = crate::auth::protect_secret(value.as_bytes()).map_err(|e| {
        ApiError::ai("CONFIG_ERROR", format!("无法加密保存密钥（Windows DPAPI 不可用）：{e}"))
    })?;
    let path = store_path(state);
    let mut store = read_store(&path);
    store.insert(reference.to_string(), crate::auth::base64_std(&sealed));
    write_store(&path, &store)?;
    Ok(describe_now(state, reference))
}

/// 删一把 key（`unset`）。文件里本来就没有它时**不写文件**（Kotlin 同样只在 `removed` 时落盘）。
pub fn unset(state: &AppState, reference: &str) -> Result<crate::ai::CredentialStatus, ApiError> {
    let path = store_path(state);
    let mut store = read_store(&path);
    if store.remove(reference).is_some() {
        write_store(&path, &store)?;
    }
    Ok(describe_now(state, reference))
}

// ---------------------------------------------------------------------------
//  密钥上行（手机 → 桌面，2026-09-27 用户拍板"双向"，docs/pairing-design.md §14）
// ---------------------------------------------------------------------------

/// 「这批 key 是哪一版」的修订号，存在 `app_settings` 里。
///
/// 为什么凭据需要一个版本号：它是**只写不读**的，没有天然的 `updated_at` 可比 ——
/// 而手机推上来的那一版可能是几天前读到的。有了它，"等值才应用"才成立：
/// 落后就回 `STALE_IGNORED`（**宁可让用户重试一次，也不许悄悄覆盖**）。
pub const REV_KEY: &str = "ai.credentials.rev";

/// 当前修订号（没有这一行 = 0，即"从没通过上行改过"）。
pub fn rev(state: &AppState) -> Result<i64, ApiError> {
    state.with(|conn| {
        let raw: Option<String> = conn
            .query_row(
                "SELECT v FROM app_settings WHERE k = ?1",
                [REV_KEY],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten();
        Ok(raw.and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(0))
    })
}

/// 修订号加一（写进 `app_settings`，用 upsert —— 被同步的表禁用 `INSERT OR REPLACE`，
/// 见红线 R3；`app_settings` 本来就不在同步范围内，但写法照纪律来）。
fn bump_rev(state: &AppState) -> Result<i64, ApiError> {
    let next = rev(state)? + 1;
    state.with(|conn| {
        conn.execute(
            "INSERT INTO app_settings (k, v, updated_at) VALUES (?1, ?2, \
             strftime('%Y-%m-%dT%H:%M:%fZ','now')) \
             ON CONFLICT(k) DO UPDATE SET v = excluded.v, updated_at = excluded.updated_at",
            rusqlite::params![REV_KEY, next.to_string()],
        )?;
        Ok(next)
    })
}

/// 一条被拒的上行（`{ref, code, message}`，与手机侧 `CredentialRejection` 同形）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub reference: String,
    /// `STALE_IGNORED` / `FORBIDDEN` / `BAD_PAYLOAD`（客户端按码给人话）。
    pub code: &'static str,
    pub message: String,
}

impl Rejection {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "ref": self.reference,
            "code": self.code,
            "message": self.message,
        })
    }
}

/// 引用名合法吗（**只判形状**：真正能不能用由上游请求说了算）。
fn valid_ref(reference: &str) -> bool {
    !reference.is_empty()
        && reference.len() <= 64
        && reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// 应用一批上行（**这是 `PUT /api/ai/credentials` 的全部业务逻辑**，单独可测）。
///
/// 三条语义（§14.2/§14.3）：
///  · `rev` 传了但**不等于**当前值 → 每一条都回 `STALE_IGNORED`，**一个字节都不写**；
///  · `rev` **不带** = 用户显式点的强制覆盖（UI 上刚点过「推给桌面」）；
///  · 逐条独立：`env` 提供的 ref 是只读（`FORBIDDEN`）、形状不合法或空值 → `BAD_PAYLOAD`，
///    其余照写 —— **一条坏的不连坐**。
pub fn apply_upload(
    state: &AppState,
    values: &BTreeMap<String, String>,
    client_rev: Option<i64>,
) -> Result<(Vec<String>, Vec<Rejection>, i64), ApiError> {
    let current = rev(state)?;
    if let Some(given) = client_rev {
        if given != current {
            let rejected = values
                .keys()
                .map(|reference| Rejection {
                    reference: reference.clone(),
                    code: "STALE_IGNORED",
                    message: format!("桌面上的凭据已经更新过（本机读到的是第 {given} 版，桌面是第 {current} 版）"),
                })
                .collect();
            return Ok((Vec::new(), rejected, current));
        }
    }

    let mut applied = Vec::new();
    let mut rejected = Vec::new();
    for (reference, raw) in values {
        if !valid_ref(reference) {
            rejected.push(Rejection {
                reference: reference.clone(),
                code: "BAD_PAYLOAD",
                message: "这个凭据名不合法（只允许字母数字与 _ - .，最长 64）".into(),
            });
            continue;
        }
        if raw.trim().is_empty() {
            // 空值**不是**"清除"：清除是桌面界面上的动作。推一个空串上来只会把好的一把抹掉。
            rejected.push(Rejection {
                reference: reference.clone(),
                code: "BAD_PAYLOAD",
                message: "空值不接受（要清除请到桌面上操作）".into(),
            });
            continue;
        }
        if env_source(reference).is_some() {
            rejected.push(Rejection {
                reference: reference.clone(),
                code: "FORBIDDEN",
                message: format!("凭据 {reference} 由环境变量提供，是只读的"),
            });
            continue;
        }
        // `set` 自己会 sanitize（带引号 / NAME=value 整行都会当场拒）；拒了就如实带出去
        match set(state, reference, raw) {
            Ok(_) => applied.push(reference.clone()),
            Err(e) => rejected.push(Rejection {
                reference: reference.clone(),
                code: "BAD_PAYLOAD",
                message: e.human().to_string(),
            }),
        }
    }
    let next = if applied.is_empty() { current } else { bump_rev(state)? };
    Ok((applied, rejected, next))
}

/// 上行回执的 JSON 形状（与手机侧 `CredentialUploadResult` 对齐）。
pub fn upload_json(applied: &[String], rejected: &[Rejection], rev: i64) -> serde_json::Value {
    serde_json::json!({
        "applied": applied,
        "rejected": rejected.iter().map(Rejection::json).collect::<Vec<_>>(),
        "rev": rev,
    })
}

#[cfg(test)]
mod tests;
