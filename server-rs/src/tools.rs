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
    /// 本 Run 的工具调用计数（**只增不减**，硬天花板见 [`ABSOLUTE_CALL_CAP`]）
    pub calls: i64,
    /// **当前这一窗**（一次回复）的调用数 —— 用户回答提问时归零（Kotlin `resetBudget`）
    pub calls_in_window: i64,
    /// 这一窗已经开过几张提问卡（阈值制：每窗最多 [`crate::questions::MAX_ASK_CARDS_PER_WINDOW`]）
    pub ask_cards_in_window: i64,
    /// 已经用掉几次"答题重置额度"（上限 [`crate::questions::MAX_BUDGET_RESETS`]）
    pub budget_resets: i64,
}

impl ToolSession {
    /// 还能不能再动一次工具（窗内额度与绝对天花板都没到）。
    pub fn has_calls_left(&self, max_calls: i64) -> bool {
        self.calls_in_window < max_calls.max(1) && self.calls < ABSOLUTE_CALL_CAP
    }

    /// 真的执行成功一次 → 消耗额度（Kotlin `spendCall`）。
    ///
    /// ⚠ **只在成功之后调**：越界被拒 / 参数不对 / 用户点拒绝 / 工具自己报错
    /// 这些"连世界都没碰到"的调用不占额度，否则用户看到的是
    /// "什么都没干就说预算不足"。
    pub fn spend_call(&mut self) {
        self.calls += 1;
        self.calls_in_window += 1;
    }

    /// 用户**真的答了**提问 → 把这一窗的额度归零，让他接着把这件事做完
    /// （用户建议 ③ 的原话："每次完成选择后，重置工具预算"）。
    ///
    /// 返回 `false` = 重置次数已经用完（那就照实回给模型，不许假装额度又满了）。
    pub fn reset_budget(&mut self) -> bool {
        if self.budget_resets >= crate::questions::MAX_BUDGET_RESETS {
            return false;
        }
        self.budget_resets += 1;
        self.calls_in_window = 0;
        self.ask_cards_in_window = 0;
        true
    }
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
/// **模型会去调一个必然失败的工具**（`TOOL_NOT_FOUND`），而界面上看不出为什么。
/// 本批是四个**只碰本机**的工具（Skills 两个 + 长期记忆一个 + 提问一个）：
/// 联网那三个（`web_search` / `web_fetch` / `civitai_search`，出厂档都是 `ASK`）
/// 要 Keenable / Civitai 客户端与 `EndpointGuard`，在下一批。
pub const IMPLEMENTED: [&str; 4] =
    ["ask_user_question", "list_skills", "load_skill", "remember"];

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
///
/// 它是 `async` 的唯一理由是 [`ask_user_question`]：那一条要**挂在这里等用户**
/// （Run 不结束）—— 其余工具都是纯本地同步逻辑，包在 async 里不改变行为。
pub async fn execute(
    state: &AppState,
    run_id: &str,
    conversation_id: &str,
    name: &str,
    arguments: &Value,
    session: &mut ToolSession,
) -> Result<ToolOutcome, ToolFailure> {
    match name {
        "list_skills" => list_skills(state),
        "load_skill" => load_skill(state, arguments, session),
        "remember" => remember(state, arguments, session),
        "ask_user_question" => {
            ask_user_question(state, run_id, conversation_id, arguments, session).await
        }
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
    // ⚠ 失败是**结构化**的（`code` + 人话），不要拿人话去 `contains("上限 100")` 反推码 ——
    // 那种猜法在新增一条错误文案时静默失效（猜错档 → 模型/界面拿到指错方向的码）。
    let (count, max, path) = crate::memory::append(&dir, &content, &today)
        .map_err(|e| ToolFailure::new(e.code, e.message))?;
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

/// 一条提问的规格（`questions[]` 与单问的 `question` 都归一成它）。
struct AskSpec {
    question: String,
    options: Vec<String>,
    multi_select: bool,
    header: Option<String>,
}

/// 把参数解析成提问规格列表（Kotlin `ToolRegistry.questionSpecs`）。
///
/// 两种写法都认：`questions[]`（多问）与旧的 `question` + `options` + `header`（单问）。
/// 空问题直接丢掉；一条都不剩时由调用方报参数错。
fn question_specs(arguments: &Value) -> Vec<AskSpec> {
    fn options_of(raw: Option<&Value>) -> Vec<String> {
        raw.and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::trim))
                    .filter(|s| !s.is_empty())
                    .take(5)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    let many: Vec<AskSpec> = arguments
        .get("questions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_object)
                .map(|q| AskSpec {
                    question: q
                        .get("question")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                    options: options_of(q.get("options")),
                    // 界面画复选框；缺省 false
                    multi_select: q.get("multiSelect").and_then(Value::as_bool).unwrap_or(false),
                    header: q
                        .get("header")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                })
                .filter(|spec| !spec.question.is_empty())
                .take(crate::questions::MAX_QUESTIONS_PER_CARD)
                .collect()
        })
        .unwrap_or_default();
    if !many.is_empty() {
        return many;
    }
    // 单问（向后兼容）
    let single = arguments
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if single.is_empty() {
        return Vec::new();
    }
    vec![AskSpec {
        question: single,
        options: options_of(arguments.get("options")),
        multi_select: arguments.get("multiSelect").and_then(Value::as_bool).unwrap_or(false),
        header: arguments
            .get("header")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    }]
}

/// 等答案时的读数：一条提问现在是什么状态（跨端可答 —— 手机点的提交也作数）。
async fn wait_for_answers(
    state: &AppState,
    run_id: &str,
    ids: &[String],
) -> Vec<Option<crate::questions::Answer>> {
    use crate::questions::QuestionState;
    let mut out: Vec<Option<crate::questions::Answer>> = vec![None; ids.len()];
    // `done[i]` = 这一题**有结论了**（答了，或者用户跳过 / 那行已失效 = 按"没回答"处理）。
    // ⚠ 与 Kotlin 的一处**有意不同**：Kotlin 的 `AskGate.discard`（跳过走它）不会唤醒
    // `awaitAll`，所以"整张卡全跳过"在那里要等到 30 分钟超时；这里把"跳过 / 失效"
    // 直接当作结论 —— 那正是 skip 端点的文档语义（"当作没回答放行，别把 Run 挂死"）。
    let mut done: Vec<bool> = vec![false; ids.len()];
    let started = std::time::Instant::now();
    let overall = std::time::Duration::from_secs(crate::questions::QUESTION_TIMEOUT_SECONDS);
    // 有**任意一条**结论之后，再给其余几条一个短宽限窗（Kotlin `awaitAll` 的降级边界：
    // 新界面一次提交整张卡；旧界面只交一题时也不至于把 Run 拖到超时）
    let mut grace_deadline: Option<std::time::Instant> = None;
    loop {
        let mut any_resolved = false;
        for (i, id) in ids.iter().enumerate() {
            if done[i] {
                any_resolved = true;
                continue;
            }
            match crate::questions::state_of(state, id) {
                Ok(QuestionState::Answered(answer)) => {
                    out[i] = Some(answer);
                    done[i] = true;
                    any_resolved = true;
                }
                // 跳过 / 已失效 / 行没了：按"用户没回答"收口（见上面那条说明）
                Ok(QuestionState::Gone) => {
                    done[i] = true;
                    any_resolved = true;
                }
                // 还在等；读库出错也当"还没答"（下一次轮询再试，不因为一次抖动丢掉整张卡）
                Ok(QuestionState::Pending) | Err(_) => {}
            }
        }
        if done.iter().all(|d| *d) {
            return out;
        }
        let now = std::time::Instant::now();
        if any_resolved && grace_deadline.is_none() {
            grace_deadline = Some(
                now + std::time::Duration::from_millis(crate::questions::DEFAULT_GRACE_MS),
            );
        }
        if state.executor.is_cancelled(run_id) {
            return out;
        }
        if now >= started + overall {
            return out;
        }
        if let Some(deadline) = grace_deadline {
            if now >= deadline {
                return out;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(crate::questions::QUESTION_POLL_MS)).await;
    }
}

/// `ask_user_question`：**向用户提问，并在原地等他回答**（不结束这一轮）。
///
/// 与审批闸门（`ToolAccess::ASK`）是两件事：审批是"允不允许"，提问是"缺的那条信息是什么"。
/// 用户**跳过**或没答时按"他没回答"处理 —— 基于已有信息做能做的、并说明缺什么，
/// **不许假装他答了，也不许原样再问一遍**（这几句会原样喂回模型）。
async fn ask_user_question(
    state: &AppState,
    run_id: &str,
    conversation_id: &str,
    arguments: &Value,
    session: &mut ToolSession,
) -> Result<ToolOutcome, ToolFailure> {
    // 阈值制的硬预算（用户 2026-09-22）：一窗最多一张卡。只做预检，
    // 计数放到**真的开了卡之后** —— 参数写错、被下面挡掉的调用不该占额度。
    if session.ask_cards_in_window >= crate::questions::MAX_ASK_CARDS_PER_WINDOW {
        return Err(ToolFailure::new(
            "ASK_BUDGET_EXCEEDED",
            format!(
                "本次回复已经开过一张提问卡了（每轮最多 {} 张）。请基于已有信息接着做，\
                 并如实说明还缺哪一条；确实还需要用户拍板，就让他直接发一条消息告诉你，\
                 而不是再开一张卡。",
                crate::questions::MAX_ASK_CARDS_PER_WINDOW
            ),
        ));
    }
    let specs = question_specs(arguments);
    if specs.is_empty() {
        return Err(ToolFailure::new(
            "INVALID_ARGUMENT",
            format!(
                "要问什么：给 questions（最多 {} 个）或 question（单个）",
                crate::questions::MAX_QUESTIONS_PER_CARD
            ),
        ));
    }
    // 同一件事问两遍：直接挡掉（不挡的话模型会连开好几张一模一样的卡片）
    for spec in &specs {
        if let Some((question_id, _)) =
            crate::questions::find_same_question(state, run_id, &spec.question)
                .map_err(failure_of)?
        {
            return Err(ToolFailure::new(
                "QUESTION_ALREADY_ASKED",
                format!(
                    "这个问题已经问过、还在等用户回答（questionId={question_id}）：「{}」。\
                     不要重复开卡片，等那一条的答案即可",
                    spec.question
                ),
            ));
        }
    }

    let group_id = crate::conversations::uuid_v4().map_err(failure_of)?;
    let total = specs.len() as i64;
    let mut records: Vec<crate::questions::StoredQuestion> = Vec::new();
    for (index, spec) in specs.iter().enumerate() {
        let question_id = crate::conversations::uuid_v4().map_err(failure_of)?;
        crate::questions::insert(
            state,
            &crate::questions::NewQuestion {
                question_id: question_id.clone(),
                run_id: run_id.to_string(),
                conversation_id: conversation_id.to_string(),
                group_id: group_id.clone(),
                index: index as i64,
                total,
                question: spec.question.clone(),
                options: spec.options.clone(),
                multi_select: spec.multi_select,
                header: spec.header.clone(),
            },
        )
        .map_err(failure_of)?;
        // 落库之后**再读回**那一行：发给界面的形状与 `GET /api/ai/questions/{id}` 完全一致
        // （少一次"手拼一份 JSON 与库里那份不一致"的机会）
        let Some(row) = crate::questions::find_stored(state, &question_id).map_err(failure_of)? else {
            return Err(ToolFailure::new("QUESTION_WRITE_FAILED", "提问落库后读不回来"));
        };
        records.push(row);
        // ⚠ 形状必须是**嵌套**的 `question` 对象（用户 bug 2026-09-21 的现场根因之一）：
        // 客户端读的是 `event.data['question']`；平铺的话那个键是**问题文字**（字符串），
        // 界面解不出提问 → 输入区不接管、工具卡一直"运行中"。
        // 一题一条事件，界面按 `groupId` 归组成同一张卡的那几页。
        state
            .bus
            .emit(
                state,
                run_id,
                crate::events::event_type::USER_QUESTION,
                &json!({ "runId": run_id, "question": records.last().unwrap().to_json() }),
            )
            .map_err(failure_of)?;
    }
    // 提问是**闸门类事件**：立即落库（另一台设备靠它发现"有东西在等人"，M1 的纪律）
    state.bus.flush(state, run_id).map_err(failure_of)?;
    session.ask_cards_in_window += 1;

    // 挂在这里等：Run **不结束**（用户建议 ⑥ 的原话）
    let ids: Vec<String> = records.iter().map(|r| r.question_id.clone()).collect();
    let answers = wait_for_answers(state, run_id, &ids).await;
    // 等待结束，没答的把库里那行按"失效"收尾（答过的已在 answer 端点写终值，
    // `finalize_if_pending` 的状态守卫保证不会覆盖）
    for id in &ids {
        let _ = crate::questions::finalize_if_pending(state, id, "expired");
    }

    // **用户答完 = 重置这一窗的工具预算**（用户建议 ③）。只有"真的答了"才算选择：
    // 全跳过 / 超时不重置，否则等于白送额度。
    let any_answered = answers.iter().any(Option::is_some);
    let budget_reset = any_answered && session.reset_budget();

    let answered_json = |record: &crate::questions::StoredQuestion, answer: &Option<crate::questions::Answer>| {
        let mut out = serde_json::Map::new();
        out.insert("questionId".into(), json!(record.question_id));
        out.insert("index".into(), json!(record.index));
        out.insert("question".into(), json!(record.question));
        out.insert("answered".into(), json!(answer.is_some()));
        if let Some(ans) = answer {
            out.insert("answer".into(), json!(ans.answer));
            if let Some(index) = ans.option_index {
                out.insert("optionIndex".into(), json!(index));
            }
            if !ans.option_indexes.is_empty() {
                out.insert("optionIndexes".into(), json!(ans.option_indexes));
            }
        }
        Value::Object(out)
    };

    let mut payload = serde_json::Map::new();
    payload.insert("groupId".into(), json!(group_id));
    payload.insert("askedCount".into(), json!(records.len()));
    payload.insert(
        "answeredCount".into(),
        json!(answers.iter().filter(|a| a.is_some()).count()),
    );
    payload.insert(
        "answers".into(),
        Value::Array(
            records
                .iter()
                .zip(answers.iter())
                .map(|(r, a)| answered_json(r, a))
                .collect(),
        ),
    );
    // 预算状态一起回给模型：它据此知道"还能接着做"，不用自己去猜
    payload.insert("callsUsedInWindow".into(), json!(session.calls_in_window));
    payload.insert("budgetResetsUsed".into(), json!(session.budget_resets));
    payload.insert("budgetReset".into(), json!(budget_reset));
    // 提问卡本身也要落块（用户建议 ⑥）：重开 App 之后那张卡要能照原样显示。
    // `asked` 是**第一条**（旧界面按单问的形状读它），多问的整组在 `askedAll`。
    payload.insert(
        "asked".into(),
        records[0]
            .with_answer(answers[0].as_ref().unwrap_or(&crate::questions::Answer {
                answer: String::new(),
                option_index: None,
                option_indexes: Vec::new(),
            }))
            .to_json(),
    );
    payload.insert(
        "askedAll".into(),
        Value::Array(
            records
                .iter()
                .zip(answers.iter())
                .map(|(r, a)| match a {
                    Some(ans) => r.with_answer(ans).to_json(),
                    None => r.to_json(),
                })
                .collect(),
        ),
    );
    // `asked` 在没答时**不该带一个空答案**（Kotlin 那边是 `?: r`，原样回问题本身）
    if answers[0].is_none() {
        payload.insert("asked".into(), records[0].to_json());
    }
    let skipped: Vec<&crate::questions::StoredQuestion> = records
        .iter()
        .zip(answers.iter())
        .filter(|(_, a)| a.is_none())
        .map(|(r, _)| r)
        .collect();
    let hint = if !any_answered {
        "用户还没有回答（可能关了界面或超时）。基于已有信息给出你能做的那部分，\
         并如实说明还缺什么；**不要**假装他答过了，也**不要**原样再问一遍。"
            .to_string()
    } else {
        let mut text = "按用户的回答接着做，不要重复提问。".to_string();
        if !skipped.is_empty() {
            text.push_str(&format!(
                "**跳过的 {} 题按\"他没回答\"处理**：{} —— 做你能做的、说明缺什么；不许替他假设一个答案。",
                skipped.len(),
                skipped.iter().map(|r| format!("「{}」", r.question)).collect::<Vec<_>>().join("、")
            ));
        }
        if budget_reset {
            text.push_str(
                "（你刚完成一次选择，本次回复的工具调用额度**已重置**，\
                 接着把这件事做完就行，不用让用户再发一条消息。）",
            );
        }
        text
    };
    payload.insert("hint".into(), json!(hint));

    // 喂回模型的正文（Kotlin 逐字）
    let content = if records.len() == 1 {
        let r = &records[0];
        match answers[0].as_ref() {
            None => format!(
                "【用户未回答】问题：「{}」\n（用户还没有回答 —— 基于已有信息继续，并如实说明缺哪一条信息。）",
                r.question
            ),
            Some(ans) => format!("【用户回答】问题：「{}」\n回答：{}", r.question, ans.answer),
        }
    } else {
        let mut text = format!("【用户回答了这张卡（共 {} 题）】\n", records.len());
        for (r, answer) in records.iter().zip(answers.iter()) {
            text.push_str(&format!(
                "\n{}. 「{}」 → {}",
                r.index + 1,
                r.question,
                match answer {
                    Some(ans) => ans.answer.clone(),
                    None => "（用户跳过 / 未回答）".to_string(),
                }
            ));
        }
        if skipped.is_empty() {
            text
        } else {
            format!(
                "{text}\n\n（标了\"跳过\"的那几题按**用户没回答**处理：\
                 基于已有信息做你能做的、并说明缺什么；不许假装他答了。）"
            )
        }
    };

    Ok(ToolOutcome::new(content, Value::Object(payload)))
}

/// 把 `ApiError` 折成工具失败（工具层的任何异常都只该让**这一次调用**失败，
/// 不该把整个 Run 打断 —— Kotlin `ToolRegistry.invoke` 的 `catch` 同一条纪律）。
fn failure_of(error: crate::routes::ApiError) -> ToolFailure {
    ToolFailure::new("TOOL_ERROR", format!("提问通道出错：{}", error.human()))
}

/// 本机日期（`YYYY-MM-DD`，按本机时区）—— 交给 SQLite 算。
fn today(state: &AppState) -> Result<String, ToolFailure> {
    state
        .with(|conn| conn.query_row("SELECT date('now','localtime')", [], |row| row.get::<_, String>(0)))
        .map_err(|e| ToolFailure::new("MEMORY_WRITE_FAILED", format!("拿不到本机日期：{}", e.human())))
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
        assert!(specs.iter().any(|s| s.name == "ask_user_question"), "提问闸门已搬，应当下发");
        // 联网那三个还没实现（要 Keenable / Civitai 客户端与 EndpointGuard）→ **不许**出现在清单里
        assert!(!specs.iter().any(|s| s.name == "web_search"));
        assert!(!specs.iter().any(|s| s.name == "web_fetch"));
        assert!(!specs.iter().any(|s| s.name == "civitai_search"));
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
