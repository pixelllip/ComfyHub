//! 界面端点第一批（P4 第 33 步）：`/api/ai/tools`、`/api/ai/tools/policy`、`/api/ai/nsfw`。
//!
//! # 为什么先搬这三条
//!
//! 它们决定"AI 到底能不能动手"，而且是**设置页那几张卡的地基**：
//! 权限档（`ask` / `full`）直接决定 `ToolAccess::Ask` 的工具要不要等用户点批准；
//! NSFW 开关决定系统提示里那一段注不注入；工具清单是那张"权限"表的行。
//! 少了它们，指向 Rust 时用户**连"自动允许"都切不了**（第 31 步做的闸门就永远拦着）。
//!
//! # 三条形状纪律（都踩过）
//!
//! 1. **可空键整个键不出现**（`explicitNulls = false`）—— `defaultWriteRoot` 只有配了才有；
//! 2. **`GET` 回的是展开后的绝对路径**（Kotlin `ToolPolicyDto.writeRoots = policy.writeRoots`，
//!    而 `ToolPolicy.writeRoots` 是 `expand(...)` 的结果），不是用户当初填的那几个字；
//! 3. **`PUT` 只改传了的字段**（其余照旧），但 `permissionMode` **非法就 400** ——
//!    悄悄回落成 `ask` 看着更"安全"，实际是把用户的意图静默改成另一种行为。
//!
//! ⚠ `autoReadRoots` 是**每次现算**的（本机 ComfyUI 目录，用户 bug ④），界面上只展示不编辑；
//! 它和用户配的 `readRoots` 是两码事，别合并。

use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::policy::{ToolPolicy, ToolPolicyConfig};
use crate::routes::{ApiError, AppState};

/// `DEFAULT_MAX_READ_BYTES`（Kotlin `ToolPolicyConfig`，2MB）—— 这两项界面上不可改，
/// Kotlin 那边 `normalizeStored` 也是**强制回落**到代码常量（实测库里冻着老值对老用户无效）。
const DEFAULT_MAX_READ_BYTES: i64 = 2 * 1024 * 1024;
/// `DEFAULT_MAX_WRITE_BYTES`（Kotlin `ToolPolicyConfig`，4MB）。
const DEFAULT_MAX_WRITE_BYTES: i64 = 4 * 1024 * 1024;

/// `GET /api/ai/tools`：这台后端**能跑**的工具（界面用它画权限表）。
///
/// ⚠ 只列 [`crate::tools::IMPLEMENTED`] —— Kotlin 那边列的是它全部 22 个 + MCP 拉来的。
/// 列一个这台后端跑不了的工具，等于告诉用户"AI 能改文件"（而调用会回 `TOOL_NOT_FOUND`）。
/// 每条六个键：`name` / `description` / `category` / `mutating` / `access` / `overridden`
/// （`AiToolInfo.fromJson` 读的就是这六个）。
pub fn list_tools(state: &AppState) -> Result<Value, ApiError> {
    let policy = ToolPolicy::load(state)?;
    let tools = crate::mobile_tools::MOBILE_TOOLS
        .iter()
        .filter(|tool| crate::tools::is_implemented(tool.name))
        .map(|tool| {
            let mut out = Map::new();
            out.insert("name".into(), Value::String(tool.name.to_string()));
            out.insert("description".into(), Value::String(tool.description.to_string()));
            out.insert("category".into(), Value::String(category_of(tool.name).to_string()));
            out.insert("mutating".into(), Value::Bool(tool.mutating));
            out.insert(
                "access".into(),
                Value::String(
                    access_wire(policy.access_for(tool.name, tool.access)).to_string(),
                ),
            );
            // 「被用户覆盖过」= 设置里有这一项（没覆盖 = 出厂默认，界面据此标"改过"）
            out.insert(
                "overridden".into(),
                Value::Bool(policy.config.overrides.contains_key(tool.name)),
            );
            Value::Object(out)
        })
        .collect();
    Ok(Value::Array(tools))
}

/// 工具类别（Kotlin `ToolCategory.wire`）—— 界面按它选图标与分组标题。
fn category_of(name: &str) -> &'static str {
    match name {
        "ask_user_question" => "ask",
        "list_skills" | "load_skill" => "skill",
        "remember" => "memory",
        "web_search" | "web_fetch" => "web",
        "civitai_search" => "web",
        _ => "comfy",
    }
}

/// 权限档 → 线上字（Kotlin `ToolAccess.wire`）。
fn access_wire(access: crate::policy::ToolAccess) -> &'static str {
    match access {
        crate::policy::ToolAccess::Allow => "allow",
        crate::policy::ToolAccess::Ask => "ask",
        crate::policy::ToolAccess::Deny => "deny",
    }
}

/// `GET /api/ai/tools/policy`（Kotlin `ToolPolicyDto`，11 个键）。
pub fn policy_json(state: &AppState) -> Result<Value, ApiError> {
    let policy = ToolPolicy::load(state)?;
    let project_root = state.project_root();
    let configured_output = configured_output_dir(state)?;
    let auto_read = crate::comfy_dirs::auto_read_roots(&project_root, configured_output.as_deref());
    Ok(policy_dto(&policy, &auto_read))
}

/// 生效的 `ToolPolicyDto`（`PUT` 之后也用它回读，"界面显示的就是库里真正生效的那份"）。
fn policy_dto(policy: &ToolPolicy, auto_read: &[PathBuf]) -> Value {
    let mut out = Map::new();
    out.insert("writeRoots".into(), paths_json(&policy.write_roots));
    out.insert("readRoots".into(), paths_json(&policy.read_roots));
    out.insert("autoReadRoots".into(), paths_json(auto_read));
    out.insert(
        "overrides".into(),
        Value::Object(
            policy
                .config
                .overrides
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect(),
        ),
    );
    out.insert("maxToolSteps".into(), Value::from(policy.config.max_tool_steps));
    out.insert("maxCallsPerRun".into(), Value::from(policy.config.max_calls_per_run));
    out.insert(
        "maxComfyQueriesPerRun".into(),
        Value::from(crate::policy::MAX_COMFY_QUERIES_PER_RUN),
    );
    out.insert("maxReadBytes".into(), Value::from(DEFAULT_MAX_READ_BYTES));
    out.insert("maxWriteBytes".into(), Value::from(DEFAULT_MAX_WRITE_BYTES));
    out.insert(
        "defaultWriteRoot".into(),
        Value::String(policy.project_root.join(crate::policy::DEFAULT_WRITE_DIR).to_string_lossy().to_string()),
    );
    out.insert("permissionMode".into(), Value::String(policy.permission_mode.clone()));
    Value::Object(out)
}

fn paths_json(paths: &[PathBuf]) -> Value {
    Value::Array(
        paths.iter().map(|p| Value::String(p.to_string_lossy().to_string())).collect(),
    )
}

/// `PUT /api/ai/tools/policy` 的请求体（Kotlin `ToolPolicyUpdate`，字段全部可缺省）。
#[derive(Debug, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PolicyUpdate {
    write_roots: Option<Vec<String>>,
    read_roots: Option<Vec<String>>,
    overrides: Option<std::collections::BTreeMap<String, String>>,
    max_tool_steps: Option<i64>,
    max_calls_per_run: Option<i64>,
    permission_mode: Option<String>,
}

/// `PUT /api/ai/tools/policy`：只改传了的字段，其余照旧；返回**回读**的生效值。
pub fn save_policy(state: &AppState, body: PolicyUpdate) -> Result<Value, ApiError> {
    // 权限档：非法值**直接拒绝**（回落成 ask 会把用户的意图静默改成另一种行为）
    let mode = match body.permission_mode.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(raw) => {
            let lower = raw.to_lowercase();
            if lower != crate::policy::PERMISSION_ASK && lower != crate::policy::PERMISSION_FULL {
                return Err(ApiError::ai(
                    "CONFIG_ERROR",
                    format!("未知的权限档：{raw}（可选 ask / full）"),
                ));
            }
            Some(lower)
        }
    };

    let current = ToolPolicy::load(state)?;
    let next = ToolPolicyConfig {
        write_roots: body.write_roots.unwrap_or_else(|| current.config.write_roots.clone()),
        read_roots: body.read_roots.unwrap_or_else(|| current.config.read_roots.clone()),
        overrides: body.overrides.unwrap_or_else(|| current.config.overrides.clone()),
        permission_mode: mode.unwrap_or_else(|| current.config.permission_mode.clone()),
        // 夹在 1..24 / 1..100（与 Kotlin 的 `coerceIn` 一致）——
        // 0 会让"一轮都不许调工具"，正整数上限之外是明摆着写错了
        max_tool_steps: body
            .max_tool_steps
            .unwrap_or(current.config.max_tool_steps)
            .clamp(1, 24),
        max_calls_per_run: body
            .max_calls_per_run
            .unwrap_or(current.config.max_calls_per_run)
            .clamp(1, 100),
    };
    save_config(state, &next)?;
    // 回读一次：界面显示的就是**库里真正生效**的那份（与 Kotlin 同）
    policy_json(state)
}

/// 把配置写进 `app_settings`（**合并**已有 JSON，保住我们这一版不认识的那些键）。
///
/// 为什么要合并而不是整份覆盖：Kotlin 那边同一行里还存着 `maxReadBytes` / `maxWriteBytes` 等
/// 字段（它自己会在读的时候强制回落成常量）。整份覆盖会把它们**悄悄删掉**，
/// 而两端来回切一次就会出现"某些设置在两个后端之间来回变"。
fn save_config(state: &AppState, next: &ToolPolicyConfig) -> Result<(), ApiError> {
    let raw: Option<String> = state.with(|conn| {
        Ok(conn
            .query_row(
                "SELECT v FROM app_settings WHERE k = ?1",
                [crate::policy::KEY],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten())
    })?;
    let mut map = raw
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();
    map.insert("writeRoots".into(), serde_json::json!(next.write_roots));
    map.insert("readRoots".into(), serde_json::json!(next.read_roots));
    map.insert("overrides".into(), serde_json::json!(next.overrides));
    map.insert("permissionMode".into(), serde_json::json!(next.permission_mode));
    map.insert("maxToolSteps".into(), serde_json::json!(next.max_tool_steps));
    map.insert("maxCallsPerRun".into(), serde_json::json!(next.max_calls_per_run));
    let text = Value::Object(map).to_string();
    state.write(move |tx| {
        tx.execute(
            "INSERT INTO app_settings (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            rusqlite::params![crate::policy::KEY, text],
        )?;
        Ok(())
    })
}

/// 配置里生效的输出目录（`auto_read_roots` 用它推"ComfyUI 在哪"）。
fn configured_output_dir(state: &AppState) -> Result<Option<String>, ApiError> {
    Ok(crate::capture::capture_config(state)?.output_dir)
}

// ---------------------------------------------------------------------------
//  NSFW 开关（`GET/PUT /api/ai/nsfw`）
// ---------------------------------------------------------------------------

/// `GET /api/ai/nsfw` / `PUT` 的响应（Kotlin `NsfwSettingsDto`，四个键）。
pub fn nsfw_json(state: &AppState) -> Result<Value, ApiError> {
    let (enabled, blur) = nsfw_config(state)?;
    Ok(serde_json::json!({
        "enabled": enabled,
        "blur": blur,
        // 真正生效的"打不打码" = 两个开关都开（**前端不要自己再算一遍**）
        "effectiveBlur": enabled && blur,
        "promptVersion": crate::nsfw_prompt::PROMPT_VERSION,
    }))
}

/// `PUT /api/ai/nsfw` 的请求体（两个字段可缺省，**缺省 = 关**）。
#[derive(Debug, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NsfwUpdate {
    enabled: Option<bool>,
    blur: Option<bool>,
}

/// `PUT /api/ai/nsfw`：写两档开关并回读。
///
/// ⚠ 缺省一律按 **false**（Kotlin `NsfwUpdate` 的注释就是这条）：漏传一个字段
/// 绝不能让那段越狱提示词被悄悄打开。
pub fn save_nsfw(state: &AppState, body: NsfwUpdate) -> Result<Value, ApiError> {
    let enabled = body.enabled.unwrap_or(false);
    let blur = body.blur.unwrap_or(false);
    let text = serde_json::json!({ "enabled": enabled, "blur": blur }).to_string();
    state.write(move |tx| {
        tx.execute(
            "INSERT INTO app_settings (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            rusqlite::params![crate::run_kit::KEY_NSFW, text],
        )?;
        Ok(())
    })?;
    nsfw_json(state)
}

/// 读 `ai.nsfw` 里那两个开关（读坏 / 没有 → 都关，与 Kotlin `NsfwConfig.load` 同口径）。
fn nsfw_config(state: &AppState) -> Result<(bool, bool), ApiError> {
    let raw: Option<String> = state.with(|conn| {
        Ok(conn
            .query_row(
                "SELECT v FROM app_settings WHERE k = ?1",
                [crate::run_kit::KEY_NSFW],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten())
    })?;
    let value = raw
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let flag = |key: &str| {
        value
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    Ok((flag("enabled"), flag("blur")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 类别与权限档的线上字与_kotlin_一致() {
        // 界面按这些字选图标 / 画下拉框，拼错就是"一个都不认识"
        assert_eq!(category_of("ask_user_question"), "ask");
        assert_eq!(category_of("list_skills"), "skill");
        assert_eq!(category_of("load_skill"), "skill");
        assert_eq!(category_of("remember"), "memory");
        assert_eq!(category_of("civitai_search"), "web");
        assert_eq!(access_wire(crate::policy::ToolAccess::Allow), "allow");
        assert_eq!(access_wire(crate::policy::ToolAccess::Ask), "ask");
        assert_eq!(access_wire(crate::policy::ToolAccess::Deny), "deny");
    }

    #[test]
    fn policy_dto_的键集合就是_kotlin_那十一个() {
        let policy = ToolPolicy::new(PathBuf::from(r"D:\proj"), ToolPolicyConfig::default());
        let json = policy_dto(&policy, &[]);
        let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "autoReadRoots",
                "defaultWriteRoot",
                "maxCallsPerRun",
                "maxComfyQueriesPerRun",
                "maxReadBytes",
                "maxToolSteps",
                "maxWriteBytes",
                "overrides",
                "permissionMode",
                "readRoots",
                "writeRoots",
            ]
        );
        // GET 回的是**展开后的绝对路径**（不是用户当初填的 "comfyui" 那几个字）
        assert_eq!(json["writeRoots"][0], r"D:\proj\comfyui");
        assert_eq!(json["defaultWriteRoot"], r"D:\proj\comfyui");
        assert_eq!(json["permissionMode"], "ask");
        assert!(json["overrides"].as_object().unwrap().is_empty());
    }
}
