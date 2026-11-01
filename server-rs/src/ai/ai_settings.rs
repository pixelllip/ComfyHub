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

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::tools::policy::{ToolPolicy, ToolPolicyConfig};
use crate::web::routes::{ApiError, AppState};

/// `DEFAULT_MAX_READ_BYTES`（Kotlin `ToolPolicyConfig`，2MB）—— 这两项界面上不可改，
/// Kotlin 那边 `normalizeStored` 也是**强制回落**到代码常量（实测库里冻着老值对老用户无效）。
///
/// 真源已挪进 [`crate::tools::policy`]（P4 第 40 步起文件工具真的按它截断）—— 这里只做转出，
/// **别在本地再写一遍数字**：两份常量漂移的表现是"界面显示 2MB、工具实际读 256KB"。
const DEFAULT_MAX_READ_BYTES: i64 = crate::tools::policy::DEFAULT_MAX_READ_BYTES;
/// `DEFAULT_MAX_WRITE_BYTES`（Kotlin `ToolPolicyConfig`，4MB）。
const DEFAULT_MAX_WRITE_BYTES: i64 = crate::tools::policy::DEFAULT_MAX_WRITE_BYTES;

/// `GET /api/ai/tools`：这台后端**能跑**的工具（界面用它画权限表）。
///
/// ⚠ 只列 [`crate::tools::tools::IMPLEMENTED`] —— Kotlin 那边列的是它全部 22 个 + MCP 拉来的。
/// 列一个这台后端跑不了的工具，等于告诉用户"AI 能改文件"（而调用会回 `TOOL_NOT_FOUND`）。
/// 每条六个键：`name` / `description` / `category` / `mutating` / `access` / `overridden`
/// （`AiToolInfo.fromJson` 读的就是这六个）。
pub fn list_tools(state: &AppState) -> Result<Value, ApiError> {
    let policy = ToolPolicy::load(state)?;
    // ⚠ 从 `tools::implemented()` 取（手机 + 桌面专属的**合并**清单，含被 `deny` 的），
    // 不是只遍历 `MOBILE_TOOLS`、也不是 `catalog`：
    // ① 第 40 步之前这条路只看手机清单，于是文件工具实现了界面也看不见 ——
    //    用户会以为"这台后端不能改文件"；
    // ② `catalog` 会把 `deny` 的剔掉，而设置页的权限表**必须能看见**被禁用的工具
    //    （剔掉的话用户点过一次禁用就再也开不回来）；
    // ③ 第 45 步起还要并上**运行时拉到的 MCP 工具**（Kotlin 那边列的就是它们，
    //    漏了的话用户在设置里根本看不到 `run_workflow` 这些能生图的工具）。
    let mut views = crate::tools::tools::implemented();
    for view in state.mcp.tools() {
        if !views.iter().any(|tool| tool.name == view.name) {
            views.push(view);
        }
    }
    views.sort_by_key(|tool| tool.name);
    let tools = views
        .into_iter()
        .map(|tool| {
            let mut out = Map::new();
            out.insert("name".into(), Value::String(tool.name.to_string()));
            out.insert("description".into(), Value::String(tool.description.to_string()));
            out.insert("category".into(), Value::String(category_of(tool.name).to_string()));
            out.insert("mutating".into(), Value::Bool(tool.mutating));
            out.insert(
                "access".into(),
                Value::String(access_wire(policy.access_for(tool.name, tool.access)).to_string()),
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
///
/// ⚠ `deny` 的工具**不在**这份清单里（`catalog` 就把它剔了），所以"界面能整类关掉"
/// 这件事只体现在 `overrides` 上，不体现在这里。
///
/// ⚠ **执行器也读它**（所以是 `pub(crate)`）：联网那档的独立预算按类别判
/// （[`crate::tools::policy::MAX_WEB_QUERIES_PER_RUN`]）。两份"哪些工具算联网"的清单迟早会漂移，
/// 而漂移的后果是**预算形同虚设**、且不会有任何用例变红 —— 所以只有这一份。
pub(crate) fn category_of(name: &str) -> &'static str {
    match name {
        "ask_user_question" => "ask",
        "list_skills" | "load_skill" | "register_skill" | "delete_skill" => "skill",
        "remember" => "memory",
        // 联网（Kotlin `ToolCategory.WEB`）：**`danbooru_lookup` 与 `find_model` 也在这里**
        // —— 两者的 Kotlin `ToolRegistry` 条目写的都是 `ToolCategory.WEB`，于是也吃
        // `MAX_WEB_QUERIES_PER_RUN` 那份预算、界面上也归在"联网"组里。
        // ⚠ 漏掉 `find_model` 的后果是**它不吃预算**（回落到 `_ => "comfy"`，
        //   变成"按 ComfyUI 查询计"，而那份额度是另一条线），且界面上分组也会错。
        "web_search" | "web_fetch" | "civitai_search" | "danbooru_lookup" | "find_model" => "web",
        // 资料库（Kotlin `ToolCategory.LIBRARY`）：查自己的提示词库 / 角色卡 / 画风卡
        "search_prompts"
        | "search_character_cards"
        | "save_character_card"
        | "search_style_cards"
        | "save_style_card"
        // 看画廊产物（2026-10-29）：读的是本机库，**不碰 ComfyUI** —— 落到下面的
        // `_ => "comfy"` 会让它白吃"不许把 ComfyUI 当轮询器"那 9 次额度。
        | "comfy_view_outputs" => "library",
        // 文件三件（Kotlin `ToolCategory.FILES`）
        "list_dir" | "read_file" | "write_file" => "files",
        // ⚠ **例外**：Kotlin 里 `comfy_env_report` 是 `ToolCategory.FILES`（它只是扫盘，
        //   不碰 ComfyUI 服务）—— 所以**不能**靠下面那条 `_ => "comfy"` 兜底，
        //   否则它一落地就会吃掉"不许把 ComfyUI 当轮询器"那 9 次额度（`MAX_COMFY_QUERIES_PER_RUN`）。
        //   它还没搬过来，这里先钉住映射，免得日后静默漂移（Kotlin `ToolRegistry.kt` 那条写的就是 FILES）。
        "comfy_env_report" => "files",
        // 训练引擎（anima-lora-next）的工具。**必须是独立类别**：落到 `comfy`
        // 兜底里会让每一次 `anima_studio_*` 都吃掉 ComfyUI 的那 9 次查询额度，
        // 而它根本不查 ComfyUI —— 表现是“问了几次训练状态，出图提交就被预算拦住”。
        _ if name.starts_with(crate::tools::mcp::ANIMA_PREFIX) => "train",
        _ => "comfy",
    }
}

/// 这件工具算不算"**主动查询** ComfyUI"—— 也就是吃不吃
/// [`crate::tools::policy::MAX_COMFY_QUERIES_PER_RUN`] 那份额度。
///
/// ⚠ **`comfy_submit` 不算**。系统提示那条规则的原话是"查 ComfyUI 状态时不要无限轮询：
/// 一次回复里最多主动**查询** ComfyUI 几次"，而提交是**动作**不是查询。
///
/// 为什么单列一条（2026-09-27 活实例实测）：把提交也算进去之后，"**让 AI 自己搭一份工作流**"
/// 这条正路直接走不通 —— 它侦察本机（读权重索引 / 查节点契约 / 搜模型：实测一轮就用了 9 次）
/// 把额度用光，等它写好工作流要提交时被 `QUERY_BUDGET_EXCEEDED` 挡住，**产物永远出不来**
/// （当时它的原话是"提交不属于查询 —— 直接跑"，而代码把它算成了查询）。
/// 反复提交本身由 `max_tool_steps` / `max_calls_per_run` 与审批闸门兜着，不需要这份额度。
pub fn is_comfy_query(name: &str) -> bool {
    category_of(name) == "comfy" && name != "comfy_submit" && !is_schema_query(name)
}

/// 只读的"节点契约"查询（comfy-mcp 的 `nodes` 一族）。
///
/// 它们问的是 `/object_info` 那份 **schema**，不是 ComfyUI 的运行状态；把它们算进
/// "别把 ComfyUI 当轮询器"那 9 次额度，现场表现是"查了两次节点类型，真要提交时
/// 预检已经没额度了"（2026-10-25 第二轮：`nodes` 两次 + 一次 check 就把额度吃光）。
/// 这里给它们**独立档**：仍受总调用数 / 轮数管，但不占那 9 次"主动查询"。
///
/// ⚠ 只放**只读**的契约查询；`run_workflow` / `launch_comfyui` 这类动作绝不进来。
fn is_schema_query(name: &str) -> bool {
    matches!(
        name,
        "nodes"
            | "node_deps"
            | "search_nodes"
            | "list_nodes"
            | "get_node"
            | "nodes_upstream"
            | "nodes_downstream"
            | "nodes_path"
            | "nodes_types"
            | "nodes_categories"
    )
}

/// 权限档 → 线上字（Kotlin `ToolAccess.wire`）。
fn access_wire(access: crate::tools::policy::ToolAccess) -> &'static str {
    match access {
        crate::tools::policy::ToolAccess::Allow => "allow",
        crate::tools::policy::ToolAccess::Ask => "ask",
        crate::tools::policy::ToolAccess::Deny => "deny",
    }
}

/// `GET /api/ai/tools/policy`（Kotlin `ToolPolicyDto`，11 个键）。
pub fn policy_json(state: &AppState) -> Result<Value, ApiError> {
    let policy = ToolPolicy::load(state)?;
    let project_root = state.project_root();
    let configured_output = configured_output_dir(state)?;
    let auto_read = crate::comfy::comfy_dirs::auto_read_roots(&project_root, configured_output.as_deref());
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
        Value::from(crate::tools::policy::MAX_COMFY_QUERIES_PER_RUN),
    );
    out.insert("maxReadBytes".into(), Value::from(DEFAULT_MAX_READ_BYTES));
    out.insert("maxWriteBytes".into(), Value::from(DEFAULT_MAX_WRITE_BYTES));
    out.insert(
        "defaultWriteRoot".into(),
        Value::String(policy.project_root.join(crate::tools::policy::DEFAULT_WRITE_DIR).to_string_lossy().to_string()),
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
            if lower != crate::tools::policy::PERMISSION_ASK && lower != crate::tools::policy::PERMISSION_FULL {
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
        // 夹在 1..`MAX_TOOL_STEPS_LIMIT` / 1..100 ——
        // 0 会让"一轮都不许调工具"，正整数上限之外是明摆着写错了。
        // ⚠ 上界**必须**跟着 `DEFAULT_MAX_TOOL_STEPS`（25）走：夹到 24 的话，
        // 设置页选 25 会被静默改回 24（界面回读才发现），而那正是"改了没生效"那一类。
        max_tool_steps: body
            .max_tool_steps
            .unwrap_or(current.config.max_tool_steps)
            .clamp(1, crate::tools::policy::MAX_TOOL_STEPS_LIMIT),
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
                [crate::tools::policy::KEY],
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
            rusqlite::params![crate::tools::policy::KEY, text],
        )?;
        Ok(())
    })
}

/// 配置里生效的输出目录（`auto_read_roots` 用它推"ComfyUI 在哪"）。
///
/// `pub(crate)`：`ToolPolicy::load` 也要用它挂"自动发现的只读目录"（P4 第 40 步）。
pub fn configured_output_dir(state: &AppState) -> Result<Option<String>, ApiError> {
    Ok(crate::comfy::capture::capture_config(state)?.output_dir)
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
        "promptVersion": crate::ai::nsfw_prompt::PROMPT_VERSION,
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
            rusqlite::params![crate::ai::run_kit::KEY_NSFW, text],
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
                [crate::ai::run_kit::KEY_NSFW],
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

// ---------------------------------------------------------------------------
//  能力目录的状态（P4 第 39 步）
// ---------------------------------------------------------------------------

/// `ai.capabilities.probe`（`ThinkingProbe.SETTING_KEY`）：是否允许实测校准（发真实请求）。
pub const KEY_PROBE: &str = "ai.capabilities.probe";

/// 实测校准开着没有（`ThinkingProbe.enabled()`：**只有 `"1"` 算开**）。
///
/// ⚠ 与 NSFW 那些开关同一条纪律：认不出来的值一律按**关**（漏一个字段绝不能让
/// "会花钱发真请求"这件事被悄悄打开）。
pub fn probe_enabled(state: &AppState) -> Result<bool, ApiError> {
    Ok(setting(state, KEY_PROBE)?.as_deref() == Some("1"))
}

/// `PUT /api/ai/capabilities/probe-toggle` 的请求体。
#[derive(Debug, Deserialize)]
pub struct ProbeToggle {
    #[serde(default)]
    pub enabled: bool,
}

/// 写"是否允许实测校准"（`ThinkingProbe.setEnabled`：写 `"1"` / `"0"`）。
pub fn save_probe_enabled(state: &AppState, enabled: bool) -> Result<(), ApiError> {
    let value = if enabled { "1" } else { "0" };
    state.write(|tx| {
        tx.execute(
            "INSERT INTO app_settings (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
            rusqlite::params![KEY_PROBE, value],
        )?;
        Ok(())
    })
}

/// 读一行 `app_settings`（认不出来 / 没有 → `None`）。
fn setting(state: &AppState, key: &str) -> Result<Option<String>, ApiError> {
    state.with(|conn| {
        Ok(conn
            .query_row("SELECT v FROM app_settings WHERE k = ?1", [key], |row| {
                row.get::<_, Option<String>>(0)
            })
            .ok()
            .flatten())
    })
}

/// `GET /api/ai/capabilities/status` —— 目录新鲜度快照（设置页 / doctor 显示用）。
///
/// 字段**拍平**（前端不用再拼）：三层各自的 source / fetchedAt / entries +
/// remote 的年龄与是否过期 + 是否允许实测校准 + 最近一次刷新的那句话。
///
/// ⚠ 三件事**不能"顺手统一"**：
/// 1. `remoteAgeHours` / `remoteStale` **与 `remote.json` 同源**（没拉过 → `remoteAgeHours`
///    整个键不出现、`remoteStale=true`）—— 写成"没拉过就 0 小时"会让界面显示"刚刚更新过"；
/// 2. `remoteFetchedAt` 是**时间列**，要走 `iso_time` 重排（Kotlin `Instant.toString()`），
///    不能原样透传；
/// 3. `lastMessage` **由调用方给**：状态接口本身不产生这句话，**键就不出现**
///    （手动刷新那条路才有话要说）。
pub fn capability_status(
    state: &AppState,
    last_message: Option<String>,
) -> Result<Value, ApiError> {
    let dir =
        crate::provider::capability_catalog::dir_of(state.media_dir().parent().unwrap_or(state.media_dir()));
    let status = crate::provider::capability_catalog::status(&dir);
    let now = state.with(|conn| {
        conn.query_row("SELECT CAST(strftime('%s','now') AS INTEGER)", [], |row| {
            row.get::<_, i64>(0)
        })
    })?;
    let dto = crate::provider::capability_catalog::CatalogStatusDto {
        version: status.version,
        builtin_rules: status.builtin_rules,
        custom_source: status.custom.as_ref().map(|m| m.source.clone()),
        custom_fetched_at: status
            .custom
            .as_ref()
            .and_then(|m| crate::infra::iso_time::iso_time(m.fetched_at.as_deref())),
        custom_entries: status.custom.as_ref().map(|m| m.entry_count),
        remote_source: status.remote.as_ref().map(|m| m.source.clone()),
        remote_fetched_at: status
            .remote
            .as_ref()
            .and_then(|m| crate::infra::iso_time::iso_time(m.fetched_at.as_deref())),
        remote_entries: status.remote.as_ref().map(|m| m.entry_count),
        remote_age_hours: crate::provider::capability_catalog::remote_age_hours(&dir, now),
        remote_stale: crate::provider::capability_catalog::is_stale(&dir, now),
        refresh_interval_days: crate::provider::capability_catalog::REFRESH_INTERVAL_DAYS,
        probe_enabled: probe_enabled(state)?,
        last_message,
    };
    serde_json::to_value(dto).map_err(|e| ApiError::internal(format!("状态序列化失败：{e}")))
}

#[cfg(test)]
mod tests;
