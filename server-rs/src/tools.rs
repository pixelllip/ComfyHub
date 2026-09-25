//! 桌面工具层（P4 第 31 步）—— 先搬 **`MOBILE` / `BOTH` 那一档里不碰网络的三个**，
//! 桌面专属的（`comfy_*` 39 个 MCP + `read_file` / `write_file` / `register_skill` …）
//! 与联网三个另起批次。
//!
//! # 为什么先搬这一批
//!
//! 桌面专属的加起来上万行：`WorkflowConvert` / `WorkflowInputs` / `WorkflowPrune` /
//! `McpStdioClient` 每一块都是独立的大工程。而 `ToolScope = MOBILE|BOTH` 那 7 个
//! （见 🔒 `docs/tool-scope-contract.md`）是**两端都能执行**的，它们的定义
//! （名字 / 描述 / inputSchema）**已经在 `mobile_tools.rs` 里**（run-kit 那一步
//! 从 Kotlin 真实 schema 生成过），所以搬它们不会出现"两端描述不一致"。
//!
//! 这一批（[`IMPLEMENTED`]）先落地三个**只碰本机**的：查技能、加载技能、记长期记忆。
//! 于是这一批之后的**桌面也能真的用工具**，而且这批的每一步都是可测的（不依赖网络、
//! 不依赖 ComfyUI、不依赖用户凭据）。联网那三个与 `ask_user_question` 留下一步：
//! `web_fetch` / `web_search` 要 Keenable 客户端与 `EndpointGuard`，
//! `civitai_search` 要 Civitai 客户端（含 NSFW 专线），`ask_user_question` 要提问闸门
//! （`ai_questions` 的写侧 + 跨端回答）。**没搬的绝不下发**（`declaredTo = kit ∩ 本地实现`）。
//!
//! # 与 Kotlin 的对应物
//!
//! | 这里 | Kotlin |
//! | --- | --- |
//! | [`execute`] | `ToolRegistry.invoke` |
//! | [`is_mutating`] / [`default_access`] | `AgentTool.mutating` / `defaultAccess` |
//! | [`ToolFailure`] | `ToolFailure(code, message)` |
//! | [`list_skills`] / [`load_skill`] / [`remember`] | 那三条同名工具 |
//!
//! # 两条纪律
//!
//! 1. **工具结果一律当不可信数据**（R7 / RSK-004）：它进的是"工具结果"那一轮，
//!    **不是**系统提示的指令区 —— 这里只负责产出文本，别在别处把它拼进指令。
//! 2. **失败要带稳定码**（`ToolFailure.code`）：界面与模型按码分支，
//!    `INVALID_ARGUMENT` / `SKILL_NOT_FOUND` / `MEMORY_FULL` 各有各的下一步。

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::mobile_tools::MOBILE_TOOLS;
use crate::routes::AppState;

/// 工具失败（Kotlin `ToolFailure`）。
#[derive(Debug, Clone)]
pub struct ToolFailure {
    /// 稳定码（`INVALID_ARGUMENT` / `SKILL_NOT_FOUND` / `MEMORY_FULL` …）
    pub code: String,
    /// 人话（这句会**原样喂回模型**，所以要写成"下一步该怎么做"）
    pub message: String,
}

impl ToolFailure {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into() }
    }
}

impl std::fmt::Display for ToolFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}：{}", self.code, self.message)
    }
}

/// 一次工具调用的结果（Kotlin `ToolOutput` + `preview`）。
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    /// 喂回模型的正文
    pub content: String,
    /// 工具卡上的摘要（Kotlin `ToolCallRecord.preview`）——**不是**正文的截断，
    /// 而是把空白压平之后取 [`MAX_PREVIEW_CHARS`]（界面上是一行小字）。
    pub preview: String,
    /// 结构化负载（落进 `ai_message_parts.json_payload` 的 `mediaIds` 等，界面按它渲染卡片）
    pub payload: Value,
}

impl ToolOutcome {
    /// 正文 + 结构化负载 → 补上 `preview`（**所有成功路径都必须走它**，
    /// 免得某个工具"忘了给 preview"而工具卡上是空白）。
    pub fn new(content: impl Into<String>, payload: Value) -> Self {
        let content = content.into();
        let preview = preview_of(&content);
        Self { content, preview, payload }
    }
}

/// 工具结果回给模型前的截断上限（Kotlin `ToolRegistry.MAX_RESULT_CHARS`）。
pub const MAX_RESULT_CHARS: usize = 32_000;
/// 工具卡摘要长度（Kotlin `MAX_PREVIEW_CHARS`）。
pub const MAX_PREVIEW_CHARS: usize = 400;
/// 单个 Run 的工具调用**硬天花板**（Kotlin `ABSOLUTE_CALL_CAP`）：
/// 正常使用一辈子碰不到，它拦的是"外挂式"无限调用。
pub const ABSOLUTE_CALL_CAP: i64 = 200;

/// 结果截断（Kotlin `truncate`）：超限时**追加一句说明**再交给模型 ——
/// 悄悄砍掉半句话，模型会以为那就是全部。
pub fn truncate_result(text: &str) -> (String, bool) {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= MAX_RESULT_CHARS {
        return (text.to_string(), false);
    }
    let head: String = chars[..MAX_RESULT_CHARS].iter().collect();
    (format!("{head}\n…（结果过长已截断，共 {} 字符）", chars.len()), true)
}

/// 工具卡摘要（Kotlin `preview`）：空白压平 + 超长补省略号。
pub fn preview_of(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    if chars.len() <= MAX_PREVIEW_CHARS {
        return flat;
    }
    let head: String = chars[..MAX_PREVIEW_CHARS].iter().collect();
    format!("{head}…")
}

/// 额度用尽时回给模型的话（Kotlin `ToolRegistry.budgetExceededNote`）。
///
/// ⚠ **措辞必须与"轮数上限"那句分开**：2026-09-21 的现场就是用户看到"已达上限 8"
/// 却以为还有 16 次可调，分不清是哪条线拦住了自己。
pub fn budget_exceeded_note(limit: i64, resettable: bool) -> String {
    if resettable {
        format!(
            "本次回复的工具调用次数已达上限（{limit} 次）。\
             如果还差最后一步，向用户说明你要做什么、让他再发一条消息即可\
             （或先提一个选择题，用户答完额度会重置）。"
        )
    } else {
        format!(
            "本次回复的工具调用总数已达硬上限（{ABSOLUTE_CALL_CAP} 次，重置额度也不再放宽）；\
             请基于已有信息作答，并把没做完的部分如实说清楚。"
        )
    }
}

/// 一次回复里已经被 `load_skill` 加载过的技能（同一个技能只返回一次正文）。
#[derive(Debug, Default)]
pub struct ToolSession {
    loaded_skills: HashSet<String>,
    /// 本轮已新增几条长期记忆（`remember` 的节流额度）
    pub memory_writes: i64,
}

/// 工具清单（**只含这一批真的实现了的**）。
///
/// ⚠ 与 `mobile_tools.rs` 的交集：只有在这里实现过的才下发给模型 ——
/// `declaredTo = kit ∩ 本地实现`（M3 的纪律）。把没实现的也发下去，
/// 模型就会去调一个必然失败的调用。
pub fn specs(policy: &crate::policy::ToolPolicy) -> Vec<crate::protocol::ToolSpec> {
    MOBILE_TOOLS
        .iter()
        .filter(|tool| IMPLEMENTED.contains(&tool.name))
        // deny 的工具根本不下发（与 run-kit 同一条规则）
        .filter(|tool| {
            policy.access_for(tool.name, tool.access) != crate::policy::ToolAccess::Deny
        })
        .map(|tool| crate::protocol::ToolSpec {
            name: tool.name.to_string(),
            description: tool.description.to_string(),
            parameters: serde_json::from_str(tool.input_schema).unwrap_or_else(|_| json!({"type":"object"})),
        })
        .collect()
}

/// 这一批真的实现了的工具（**唯一真源**，`specs` 与 `execute` 都从它派生）。
///
/// ⚠ 加一个名字之前先确认 [`execute`] 里真的有那条分支 —— 两个清单漂移的后果是
/// **模型会去调一个必然失败的工具**（`TOOL_NOT_AVAILABLE`），而界面上看不出为什么。
/// 本批是三个**只碰本机**的工具（Skills 两个 + 长期记忆一个）：
/// 联网那三个（`web_search` / `web_fetch` / `civitai_search`，出厂档都是 `ASK`）
/// 与 `ask_user_question`（要提问闸门）在下一批。
pub const IMPLEMENTED: [&str; 3] = ["list_skills", "load_skill", "remember"];

/// 这个工具会不会写数据（系统提示里"（会写数据）"那个标记的真源）。
pub fn is_mutating(name: &str) -> bool {
    MOBILE_TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.mutating)
        .unwrap_or(false)
}

/// 出厂权限档（`ALLOW` / `ASK` / `DENY`）。
pub fn default_access(name: &str) -> &'static str {
    MOBILE_TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.access)
        .unwrap_or("ASK")
}

/// 工具是否已实现（没实现的**不许下发**，也不许执行）。
pub fn is_implemented(name: &str) -> bool {
    IMPLEMENTED.contains(&name)
}

/// 执行一次工具调用。
///
/// `arguments` 是模型给的那份 JSON（**当不可信数据**：每个字段都要自己校验，
/// 缺字段一律 [`ToolFailure`] 而不是 `unwrap`）。
///
/// ⚠ 调用方（`executor::handle_call`）在进这里之前已经做过四道闸门：
/// 工具名在不在清单里（`TOOL_NOT_FOUND`）、策略是不是 `deny`（`TOOL_DENIED`）、
/// 额度还剩不剩（`TOOL_BUDGET_EXCEEDED`）、要不要用户批准（R8）。**这里只管执行**。
pub fn execute(
    state: &AppState,
    name: &str,
    arguments: &Value,
    session: &mut ToolSession,
) -> Result<ToolOutcome, ToolFailure> {
    match name {
        "list_skills" => list_skills(state),
        "load_skill" => load_skill(state, arguments, session),
        "remember" => remember(state, arguments, session),
        // 清单里没有的（模型幻觉出一个名字 / 这一批还没搬的）——
        // 码与 Kotlin `TOOL_NOT_FOUND` 一致，模型的分支行为才不会两端不同。
        _ => Err(ToolFailure::new(
            "TOOL_NOT_FOUND",
            format!(
                "没有名为「{name}」的工具；可用工具见系统提示里的清单\
                 （P4 后端还没搬的工具一律不在那份清单里）"
            ),
        )),
    }
}

/// 长文本参数（缺字段 / 不是字符串 → `INVALID_ARGUMENT`，**不 unwrap**）。
fn string_arg(arguments: &Value, key: &str) -> Result<String, ToolFailure> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ToolFailure::new("INVALID_ARGUMENT", format!("缺少参数 {key}")))
}

/// Skills 的两个根目录（与 `run_kit` / `skills::snapshot` **同一个算法**）。
fn skill_roots(state: &AppState) -> (PathBuf, PathBuf) {
    let storage = state.media_dir().parent().unwrap_or(state.media_dir()).to_path_buf();
    (state.project_root().join("skills").join("builtin"), storage.join("ai").join("skills"))
}

/// 长期记忆所在目录。
fn memory_dir(state: &AppState) -> PathBuf {
    state.media_dir().parent().unwrap_or(state.media_dir()).join("ai")
}

/// `list_skills`：**连不合法的一起列出来**（带 `validationError`）——
/// 静默忽略会让用户看到"装了个 skill 却没生效"而没有任何线索（AIH-038）。
fn list_skills(state: &AppState) -> Result<ToolOutcome, ToolFailure> {
    let entries = scan_all(state);
    let skills: Vec<Value> = entries
        .into_iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "whenToUse": s.when_to_use,
                "source": s.source,
                "available": s.valid,
                "validationError": if s.valid { Value::Null } else { json!(s.error) },
            })
        })
        .collect();
    let payload = json!({ "count": skills.len(), "skills": skills });
    Ok(ToolOutcome::new(payload.to_string(), payload))
}

/// `load_skill`：正文从**本地磁盘**读（不缓存），同一个技能一轮只返回一次。
fn load_skill(
    state: &AppState,
    arguments: &Value,
    session: &mut ToolSession,
) -> Result<ToolOutcome, ToolFailure> {
    let name = string_arg(arguments, "name")?;
    let name = name.trim().to_string();
    if !session.loaded_skills.insert(name.clone()) {
        return Err(ToolFailure::new(
            "SKILL_ALREADY_LOADED",
            format!("skill「{name}」在本次回复里已经加载过了，直接按它的规则继续"),
        ));
    }
    let (builtin, user) = skill_roots(state);
    let Some(found) = crate::skills::read_body(&builtin, &user, &name) else {
        return Err(ToolFailure::new(
            "SKILL_NOT_FOUND",
            format!("没有名为「{name}」的 skill；先用 list_skills 看有哪些"),
        ));
    };
    if !found.valid {
        return Err(ToolFailure::new(
            "SKILL_INVALID",
            format!("skill「{name}」不合法，不能加载：它的 frontmatter 或正文没通过校验"),
        ));
    }
    let entry = found.entry;
    // 形状照抄 Kotlin：正文包在 `<skill_content name="…" version="…">` 里
    // （`version` 只在 frontmatter 写了的时候出现 —— 这是模型判断"加载的是哪一版"的唯一线索）
    let mut open = format!("<skill_content name=\"{}\"", entry.name);
    if let Some(version) = &entry.version {
        open.push_str(&format!(" version=\"{version}\""));
    }
    open.push_str(">\n");
    let content = format!("{open}{}\n</skill_content>", found.body);
    Ok(ToolOutcome::new(
        content,
        json!({ "name": entry.name, "digest": found.digest }),
    ))
}

/// `remember`：写长期记忆（**超限一律报错，不许静默截断** —— AGENTS §10）。
fn remember(
    state: &AppState,
    arguments: &Value,
    session: &mut ToolSession,
) -> Result<ToolOutcome, ToolFailure> {
    let content = string_arg(arguments, "content")?;
    let dir = memory_dir(state);
    // 判重命中的"又说了一遍"不算新增：既不占额度、也不该被节流挡住
    if !crate::memory::has_entry(&dir, &content)
        && session.memory_writes >= crate::memory::MAX_ENTRIES_PER_RUN
    {
        return Err(ToolFailure::new(
            "MEMORY_RATE_LIMITED",
            format!(
                "本次回复已经新增了 {} 条记忆（每轮上限 {} 条）。挑最要紧的留下，其余的等用户明确让你记再写。",
                session.memory_writes,
                crate::memory::MAX_ENTRIES_PER_RUN
            ),
        ));
    }
    let before = crate::memory::count_entries(&crate::memory::read(&dir));
    let today = today(state)?;
    let (count, max, path) = crate::memory::append(&dir, &content, &today)
        .map_err(|message| ToolFailure::new(code_of_memory_error(&message), message))?;
    if count > before {
        session.memory_writes += 1;
    }
    let payload = json!({ "entryCount": count, "maxEntries": max, "path": path });
    Ok(ToolOutcome::new(
        format!(
            "已记住（长期记忆现有 {count}/{max} 条，本次回复已记 {} 条）。\
             用户可以在 AI 工作台右侧栏的「长期记忆」里搜索、修改或删除。",
            session.memory_writes
        ),
        payload,
    ))
}

/// 本机日期（`YYYY-MM-DD`，按本机时区）—— 交给 SQLite 算。
fn today(state: &AppState) -> Result<String, ToolFailure> {
    state
        .with(|conn| conn.query_row("SELECT date('now','localtime')", [], |row| row.get::<_, String>(0)))
        .map_err(|e| ToolFailure::new("MEMORY_WRITE_FAILED", format!("拿不到本机日期：{}", e.human())))
}

/// 记忆写入的失败文本 → 稳定码（按 Kotlin 的那几个码分档）。
fn code_of_memory_error(message: &str) -> &'static str {
    if message.contains("上限 100") {
        "MEMORY_FULL"
    } else if message.contains("字符上限") {
        "MEMORY_TOO_LARGE"
    } else if message.contains("太长") || message.contains("不能为空") {
        "INVALID_ARGUMENT"
    } else {
        "MEMORY_WRITE_FAILED"
    }
}

/// 扫全量（含非法项）—— `list_skills` 用它。
///
/// 为什么不复用 `skills::catalog`：那个**刻意**只给能用的（它进系统提示），
/// 而 `list_skills` 要的是"装了但有问题"的那些 —— 静默过滤掉正是 AIH-038 要防的事。
fn scan_all(state: &AppState) -> Vec<crate::skills::ScannedSkill> {
    let (builtin, user) = skill_roots(state);
    crate::skills::scan_all(&builtin, &user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_only_include_implemented_tools() {
        let policy = crate::policy::ToolPolicy::new(
            std::env::temp_dir(),
            crate::policy::ToolPolicyConfig::default(),
        );
        let specs = specs(&policy);
        for spec in &specs {
            assert!(
                IMPLEMENTED.contains(&spec.name.as_str()),
                "{} 没实现却下发了 —— 模型会去调一个必然失败的调用",
                spec.name
            );
            assert!(!spec.description.is_empty());
            assert!(spec.parameters.is_object(), "{} 的 schema 必须是对象", spec.name);
        }
        assert!(specs.iter().any(|s| s.name == "list_skills"));
        // 联网那三个还没实现（要 Keenable / Civitai 客户端）→ **不许**出现在清单里
        assert!(!specs.iter().any(|s| s.name == "web_search"));
        assert!(!specs.iter().any(|s| s.name == "web_fetch"));
        assert!(!specs.iter().any(|s| s.name == "civitai_search"));
        // 提问闸门也还没搬（`ai_questions` 的写侧在下一批）→ 同样不下发
        assert!(!specs.iter().any(|s| s.name == "ask_user_question"));
    }

    #[test]
    fn missing_arguments_are_reported_not_unwrapped() {
        let err = string_arg(&json!({}), "name").unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
        assert!(err.message.contains("name"));
        // 类型不对也算缺（模型给 null / 数字都要挡住）
        assert!(string_arg(&json!({"name": 7}), "name").is_err());
        assert!(string_arg(&json!({"name": null}), "name").is_err());
        assert_eq!(string_arg(&json!({"name": "x"}), "name").unwrap(), "x");
    }

    #[test]
    fn memory_error_codes_are_stable() {
        assert_eq!(code_of_memory_error("长期记忆已经有 100 条（上限 100 条）。"), "MEMORY_FULL");
        assert_eq!(code_of_memory_error("长期记忆已满（8000 字符上限）"), "MEMORY_TOO_LARGE");
        assert_eq!(code_of_memory_error("一条记忆太长（501 > 500 字符）"), "INVALID_ARGUMENT");
        assert_eq!(code_of_memory_error("写记忆文件失败：拒绝访问"), "MEMORY_WRITE_FAILED");
    }

    #[test]
    fn mutating_flags_come_from_the_generated_catalog() {
        assert!(is_mutating("remember"));
        assert!(!is_mutating("list_skills"));
        assert_eq!(default_access("civitai_search"), "ASK");
        assert_eq!(default_access("list_skills"), "ALLOW");
        // 不认识的工具：按最保守的来（ASK）
        assert_eq!(default_access("nope"), "ASK");
        assert!(!is_implemented("web_search"));
        assert!(!is_implemented("civitai_search"));
        assert!(is_implemented("remember"));
    }
}
