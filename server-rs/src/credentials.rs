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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_照_kotlin_的四条判据() {
        // 正常值：**trim 后**存
        assert_eq!(sanitize("  sk-abc  ").unwrap(), "sk-abc");
        // `=` 前的段不是合法标识符（带 `-`）→ 不当成 NAME=value，原样收下
        assert_eq!(sanitize("sk-a=b").unwrap(), "sk-a=b");

        // 空白
        let e = sanitize("   ").unwrap_err();
        assert_eq!(e.error, "CONFIG_ERROR");
        assert_eq!(e.detail.as_deref(), Some("密钥不能为空；留空表示不修改已有密钥"));

        // 带引号（含"单个引号字符"这种边界 —— Kotlin 的 startsWith/endsWith 也拒它）
        for raw in ["\"sk-abc\"", "'sk-abc'", "\"", "'"] {
            let e = sanitize(raw).unwrap_err();
            assert_eq!(e.detail.as_deref(), Some("请只粘贴密钥本身，不要带引号"), "{raw}");
        }
        // 一边一个引号不算"带引号"（Kotlin 只认首尾同款）
        assert_eq!(sanitize("\"sk-abc'").unwrap(), "\"sk-abc'");

        // NAME=value 整行
        let e = sanitize("OPENAI_API_KEY=sk-abc").unwrap_err();
        assert_eq!(e.detail.as_deref(), Some("请只粘贴密钥的值，不要粘贴 NAME=value 这种整行"));
        // `^[A-Za-z_][A-Za-z0-9_]*$` 也认得下划线开头的名字
        assert_eq!(
            sanitize("_A1=x").unwrap_err().detail.as_deref(),
            Some("请只粘贴密钥的值，不要粘贴 NAME=value 这种整行")
        );
        // 但数字开头的**不是**合法标识符 → 当普通值放行（Kotlin 同）
        assert_eq!(sanitize("9A=x").unwrap(), "9A=x");

        // 不可打印 / 含空格 / 非 ASCII
        for raw in ["sk abc", "sk\tabc", "sk abc\ndef", "密钥"] {
            let e = sanitize(raw).unwrap_err();
            // 含换行的值会被 trim 成 "sk abc" 之类，但仍应被拒（空格）
            assert_eq!(
                e.detail.as_deref(),
                Some("密钥只能包含可打印字符，且不能有空格或换行"),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn 读坏文件当空而不是报错() {
        let dir = std::env::temp_dir().join("comfyhub-cred-missing");
        assert!(read_store(&dir.join("nope.json")).is_empty());

        let bad = std::env::temp_dir().join(format!("comfyhub-cred-bad-{}.json", std::process::id()));
        std::fs::write(&bad, "{ 这不是 JSON").unwrap();
        assert!(read_store(&bad).is_empty());
        // 值的类型不是字符串时按 Kotlin 的 JsonPrimitive.content 取字面量；对象/数组跳过
        std::fs::write(&bad, r#"{"a":"x","n":5,"b":true,"o":{},"arr":[]}"#).unwrap();
        let store = read_store(&bad);
        assert_eq!(store.get("a").map(String::as_str), Some("x"));
        assert_eq!(store.get("n").map(String::as_str), Some("5"));
        assert_eq!(store.get("b").map(String::as_str), Some("true"));
        assert!(!store.contains_key("o"));
        assert!(!store.contains_key("arr"));
        let _ = std::fs::remove_file(&bad);
    }
}
