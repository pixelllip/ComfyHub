//! 桌面工具层（P4 第 31 步建地基，第 40 步补桌面文件工具五件，第 41 步补 comfy 查询两件）。
//!
//! # 现在这一层有什么
//!
//! | 工具 | 端 | 说明 |
//! | --- | --- | --- |
//! | `list_skills` / `load_skill` / `remember` / `ask_user_question` | `BOTH` | 第 31/32 步（只碰本机） |
//! | `list_dir` / `read_file` / `write_file` / `register_skill` / `delete_skill` | `DESKTOP` | 第 40 步，路径准入在 [`crate::policy`] |
//! | `comfy_get_run` / `comfy_list_workflows` / `comfy_get_status` | `DESKTOP` | 第 41/42 步；前两条只碰本机库与磁盘，`get_status` 会真问一次 ComfyUI 的 `/queue` |
//! | `comfy_sync_history` | `DESKTOP` | 第 43 步；**会写库**（建提示词 + 产物入库），出厂档 `ASK` |
//! | `comfy_load_workflow` | `DESKTOP` | 第 44 步；把本机工作流文件读进库（界面格式按 `/object_info` 转 API 图）。⚠ 组节点不展开、"模拟用户行为"未实现，两者都如实报 |
//! | `comfy_use_media` / `comfy_use_attachment` / `comfy_make_mask` | `DESKTOP` | **图像编辑三件**（见 [`crate::image_edit`]）：把画廊产物 / 用户附件投放进 ComfyUI 的 `input` 目录，以及生成"白=重绘区"的遮罩。三件都要用户批准（会往用户机器上写文件） |
//! | `comfy_check_inputs` / `comfy_fit_workflow` | `DESKTOP` | 第 62 步的**体检两件**（见 [`crate::workflow_fit`]）：前者列"还等着外部给输入的位置"（用户没给参考图时的那个现场），后者把图里点名的模型与本机清单对一遍。两件都**只报不改**、出厂 `ALLOW` |
//! | `comfy_audit_workflow` | `DESKTOP` | 第 63 步的**审核**（见 [`crate::workflow_audit`]）：七项检查只报事实，其中"必跑不了"的三项（前端节点 / 版本不兼容 / 本机没有的节点类）会被提交闸门拦下，参数/显存/提示词风格**只提醒**。出厂 `ALLOW` |
//! | `comfy_prune_workflow` | `DESKTOP` | 第 63 步的**摘节点 / 拼节点**（见 [`crate::workflow_prune`]）：从输出节点反向做可达性分析摘掉无关分支并把下游的线接回上游，`analyze=true` 时一个字都不改。出厂 `ALLOW`（它不写盘、不提交，改好的图是**内联**返回的） |
//! | `comfy_learn_workflow` / `comfy_list_recipes` / `comfy_bind_recipe` | `DESKTOP` | 第 67 步的**配方三件**（见 [`crate::workflow_recipe`]）：把一份具体工作流**归纳**成「任务 + 骨架指纹 + 模型槽位 + 参数旋钮」并存进本机配方库，新模型出现时只做"把骨架绑到新槽位"这一步。学与绑都与 `POST /api/ai/workflow-recipes*` **共用一份实现**；绑**只报不改**（真提交仍走 `comfy_submit`，R10 不动） |
//! | `web_search` / `web_fetch` | `BOTH` | 第 64 步的**联网两件**（见 [`crate::web_tools`]）：Keenable 通用搜索（默认免密钥）+ 取公开网页正文。`web_fetch` 的目标过 SSRF 闸门，内网 / 本机 / 云元数据地址一个请求都不发。两件都出厂 `ASK` |
//! | `civitai_search` | `BOTH` | 第 64 步的**模型库检索**（见 [`crate::civitai`]）：安全档走 `civitai.com`、`allowNsfw=true` 切到 `civitai.red`；没令牌也照搜，但正文里如实写明"本次是匿名通道"。出厂 `ASK` |
//! | `find_model` | `DESKTOP` | 第 64 步的**定向查询**（见 [`crate::find_model`]）：先查本机（命中就一个外部请求都不发），再按 Civitai → HuggingFace → ModelScope 顺序查、逐源报可达性。**绝不代下**（只给链接 + sha256 + 目标目录）。出厂 `ASK` |
//!
//! 还没搬的：官方 comfy-mcp 那 39 个（要 `McpStdioClient`，运行时拉的，不进这份静态清单）。
//! **没搬的绝不下发**（`declaredTo = 清单 ∩ 本地实现`）。
//!
//! # 两个清单的差别（别混用）
//!
//! - [`catalog`]：**下发给模型**的（剔除 `deny`）—— `specs` 与系统提示都从它派生；
//! - [`implemented`]：**设置页**看的（含被 `deny` 的）—— 剔掉的话用户点过一次禁用
//!   就再也开不回来。
//!
//! # 与 Kotlin 的对应物
//!
//! | 这里 | Kotlin |
//! | --- | --- |
//! | [`execute`] | `ToolRegistry.invoke` |
//! | [`is_mutating`] / [`default_access`] | `AgentTool.mutating` / `defaultAccess` |
//! | [`list_skills`] / [`load_skill`] / [`remember`] | 那三条同名工具 |
//! | [`list_dir`] / [`read_file`] / [`write_file`] | 那三条同名工具（含字符边界与分段读） |
//!
//! # 两条纪律
//!
//! 1. **工具结果一律当不可信数据**（R7 / RSK-004）：它进的是"工具结果"那一轮，
//!    **不是**系统提示的指令区 —— 这里只负责产出文本，别在别处把它拼进指令。
//! 2. **失败要带稳定码**（`ToolFailure.code`）：界面与模型按码分支，
//!    `INVALID_ARGUMENT` / `SKILL_NOT_FOUND` / `MEMORY_FULL` / `PATH_DENIED` 各有各的下一步。

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::{json, Value};

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
/// 资料库类工具一次最多回几条（Kotlin `MAX_LIBRARY_RESULTS`）。
pub const MAX_LIBRARY_RESULTS: i64 = 20;
/// 资料库类工具里"提示词正文"每一段的截断上限（Kotlin `MAX_PROMPT_CHARS`）。
pub const MAX_PROMPT_CHARS: usize = 1500;
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

/// 逐字符截断（Kotlin `clip`）—— 资料库类工具给模型看的那份会很长，
/// **按字符切**（按字节切会把中文切成半个字）。
pub fn clip_text(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let head: String = chars[..max].iter().collect();
    format!("{head}…（已截断，共 {} 字）", chars.len())
}

/// 资料库类工具的 `ApiError` → 工具失败（码与 Kotlin 那边一致：`NOT_FOUND` / `TOOL_ERROR`）。
fn library_failure(error: crate::routes::ApiError) -> ToolFailure {
    use axum::http::StatusCode;
    let code = match error.status {
        StatusCode::BAD_REQUEST => "INVALID_ARGUMENT",
        StatusCode::NOT_FOUND => "NOT_FOUND",
        _ => "TOOL_ERROR",
    };
    ToolFailure::new(code, error.human().to_string())
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
    /// 这一窗里**真跑成功**的联网类工具的调用数（Kotlin `ToolContext.webInWindow`）。
    ///
    /// 与 [`crate::policy::MAX_WEB_QUERIES_PER_RUN`] 一起构成"不许拿外部服务当轮询器"这条线：
    /// 每搜一次都是一次真实的外部请求 + 一份要烧 token 的正文，"再搜一下确认"很容易刹不住车。
    pub web_queries_in_window: i64,
    /// 这一窗里**真跑成功**的 ComfyUI 类工具的调用数（Kotlin `ToolContext.comfyInWindow`）。
    ///
    /// 与 [`crate::policy::MAX_COMFY_QUERIES_PER_RUN`] 一起构成 **AIH-036「不许把 ComfyUI
    /// 当轮询器」** 这条线：`comfy_get_status` / `comfy_list_workflows` / `comfy_get_run`
    /// 这类查询便宜又顺手，模型很容易"再确认一下"刷十几次。
    /// 判据是**类别**（[`crate::ai_settings::category_of`] 回 `"comfy"`），不是名字前缀 ——
    /// 与界面分组、与联网那份预算都是同一份清单。
    pub comfy_queries_in_window: i64,
    /// 这一窗已经开过几张提问卡（阈值制：每窗最多 [`crate::questions::MAX_ASK_CARDS_PER_WINDOW`]）
    pub ask_cards_in_window: i64,
    /// 已经用掉几次"答题重置额度"（上限 [`crate::questions::MAX_BUDGET_RESETS`]）
    pub budget_resets: i64,
    /// **这一轮真的查过 danbooru 的 tag**（Kotlin `ToolContext.danbooruVerified`）。
    ///
    /// 为什么要有它：`save_character_card` 的 `tagsVerifiedAt` 是"这些 tag 跟 danbooru
    /// 核对过"的**唯一凭据** —— 它只能由"真去查过"这件事写，**绝不能由模型自称**
    /// （否则那张卡会带着一个假的核对时间，而假时间比没有时间更坏）。
    /// 键是 [`crate::danbooru::normalize_tag`] 归一之后的写法（`shinku_(nte)`）。
    danbooru_verified: HashSet<String>,
    /// **本次请求是否带了图片附件**（Kotlin `ToolContext.hasImageAttachment`）。
    ///
    /// 与系统提示里那句"本次请求附带图片附件"**是同一个事实**（Kotlin 那边也是同一个
    /// `hasInlineAttachments` 同时喂给两处），所以由执行器在建 session 时一次性钉住。
    /// `comfy_check_inputs` 靠它区分"投放一下就能解决"与"必须问用户要一张"——
    /// 少了这一位，模型会去要一张它已经收到的图。
    pub has_image_attachment: bool,
}

impl ToolSession {
    /// 记下"这一轮真查过这个 tag"（归一之后进集合；空串直接忽略）。
    pub fn mark_danbooru_verified(&mut self, tag: &str) {
        let key = crate::danbooru::normalize_tag(tag);
        if !key.is_empty() {
            self.danbooru_verified.insert(key);
        }
    }

    /// 这一轮查过这个 tag 吗（`save_character_card` 的判据）。
    pub fn danbooru_verified(&self, tag: &str) -> bool {
        self.danbooru_verified.contains(&crate::danbooru::normalize_tag(tag))
    }

    /// 还能不能再动一次工具（窗内额度与绝对天花板都没到）。
    pub fn has_calls_left(&self, max_calls: i64) -> bool {
        self.calls_in_window < max_calls.max(1) && self.calls < ABSOLUTE_CALL_CAP
    }

    /// 真的执行成功一次 → 消耗额度（Kotlin `spendCall`）。
    ///
    /// ⚠ **只在成功之后调**：越界被拒 / 参数不对 / 用户点拒绝 / 工具自己报错
    /// 这些"连世界都没碰到"的调用不占额度，否则用户看到的是
    /// "什么都没干就说预算不足"。
    ///
    /// `name` 用来分开记"联网那几件"的次数（Kotlin `spendCall` 里按 `tool.category` 分档）——
    /// 联网额度是**独立的一条线**（[`crate::policy::MAX_WEB_QUERIES_PER_RUN`]），
    /// 不与总调用额度混在一个计数里。
    pub fn spend_call(&mut self, name: &str) {
        self.calls += 1;
        self.calls_in_window += 1;
        match crate::ai_settings::category_of(name) {
            "web" => self.web_queries_in_window += 1,
            // ⚠ `comfy_submit` 不算"查询"（见 `ai_settings::is_comfy_query` 的注释：
            //   把它算进去会让"AI 自己搭工作流"那条路走不通）
            "comfy" => {
                if crate::ai_settings::is_comfy_query(name) {
                    self.comfy_queries_in_window += 1;
                }
            }
            _ => {}
        }
    }

    /// 这一窗的联网额度还剩不剩（预检用；**扣费在真跑成功之后**）。
    pub fn has_web_queries_left(&self) -> bool {
        self.web_queries_in_window < crate::policy::MAX_WEB_QUERIES_PER_RUN
    }

    /// 这一窗的 ComfyUI 查询额度还剩不剩（同上；AIH-036）。
    pub fn has_comfy_queries_left(&self) -> bool {
        self.comfy_queries_in_window < crate::policy::MAX_COMFY_QUERIES_PER_RUN
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
        // 联网额度也归零：与 Kotlin 的 `webInWindow` 同一档（用户答完接着做同一件事）
        self.web_queries_in_window = 0;
        // ComfyUI 那档同理（Kotlin `resetBudget` 清的也是同一批窗内计数）
        self.comfy_queries_in_window = 0;
        true
    }
}

/// 工具清单（**只含真的实现了的**）。
///
/// ⚠ 两个来源合并：手机可执行的（[`MOBILE_TOOLS`]，两端都能跑的那 7 个）与
/// **桌面专属**的（[`crate::desktop_tools::DESKTOP_TOOLS`]，P4 第 40 步起是文件工具五件）。
/// 合并之后仍按 name 升序 —— 系统提示里那两行与下发的 `tools[]` 顺序都从这里来。
///
/// ⚠ 只有 [`is_implemented`] 为真的才下发（`declaredTo = 清单 ∩ 本地实现`，M3 的纪律）：
/// 把没实现的也发下去，模型就会去调一个必然失败的调用。
pub fn specs(policy: &crate::policy::ToolPolicy) -> Vec<crate::protocol::ToolSpec> {
    catalog(policy)
        .into_iter()
        .map(|tool| spec_of_view(&tool))
        .collect()
}

/// 一条工具视图 → 下发给上游的 `ToolSpec`。
///
/// `input_schema` 是**原样透传**的那份 JSON Schema（MCP 的也一样，见 `mcp.rs` 模块头 §2）：
/// 解析不出来时退化成空对象 schema，**不编一份我们自己的形状**（改错了模型就调不对参数）。
pub fn spec_of_view(view: &ToolView<'_>) -> crate::protocol::ToolSpec {
    crate::protocol::ToolSpec {
        name: view.name.to_string(),
        description: view.description.to_string(),
        parameters: serde_json::from_str(view.input_schema).unwrap_or_else(|_| json!({"type":"object"})),
    }
}

/// 一个工具在"清单"这一层的借用视图（两种来源的公共形状）。
///
/// 为什么要有它：`system_prompt::render` 要的是"名字 / 描述 / 出厂档 / 是否写数据"这四样，
/// 而它拿到的可能是手机清单（run-kit 那条路，必须与 Kotlin 逐字一致）也可能是合并清单
/// （桌面执行器那条路）。**别在 render 里分叉**，那会让两端提示词的拼法慢慢走偏。
#[derive(Debug, Clone, Copy)]
pub struct ToolView<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub input_schema: &'a str,
    pub access: &'a str,
    pub mutating: bool,
}

impl<'a> From<&'a crate::mobile_tools::MobileToolSpec> for ToolView<'a> {
    fn from(tool: &'a crate::mobile_tools::MobileToolSpec) -> Self {
        Self {
            name: tool.name,
            description: tool.description,
            input_schema: tool.input_schema,
            access: tool.access,
            mutating: tool.mutating,
        }
    }
}

impl<'a> From<&'a crate::desktop_tools::DesktopToolSpec> for ToolView<'a> {
    fn from(tool: &'a crate::desktop_tools::DesktopToolSpec) -> Self {
        Self {
            name: tool.name,
            description: tool.description,
            input_schema: tool.input_schema,
            access: tool.access,
            mutating: tool.mutating,
        }
    }
}

/// 下发给模型的工具清单（已实现 ∩ 未被 `deny`），**按 name 升序**。
///
/// `specs` 与 `system_prompt::render` 都从这里派生 —— 两边各拼一份的话，
/// "提示词里说能调、请求里没有这个工具"这种不一致会**静默**存在。
pub fn catalog(policy: &crate::policy::ToolPolicy) -> Vec<ToolView<'static>> {
    let mut out = implemented();
    out.retain(|tool| {
        policy.access_for(tool.name, tool.access) != crate::policy::ToolAccess::Deny
    });
    out
}

/// 这台后端**真的能跑**的工具（含被 `deny` 的那些），按 name 升序。
///
/// ⚠ 与 [`catalog`] 的差别只有一条：**这里不剔 `deny`**。设置页的权限表要能看见
/// "哪些被禁用了"并把它改回来 —— 剔掉的话用户一旦点过一次禁用，那个工具就从界面上
/// 消失、再也开不回来（界面按 `access == "deny"` 画灰）。
/// 而**下发给模型**的那份必须剔（`catalog`）—— 两份别混用。
pub fn implemented() -> Vec<ToolView<'static>> {
    let mut out: Vec<ToolView<'static>> = Vec::new();
    for tool in crate::mobile_tools::MOBILE_TOOLS.iter() {
        if is_implemented(tool.name) {
            out.push(ToolView::from(tool));
        }
    }
    for tool in crate::desktop_tools::DESKTOP_TOOLS.iter() {
        if is_implemented(tool.name) {
            out.push(ToolView::from(tool));
        }
    }
    out.sort_by_key(|tool| tool.name);
    out
}

/// 这一批真的实现了的工具（**唯一真源**，`specs` 与 `execute` 都从它派生）。
///
/// ⚠ 加一个名字之前先确认 [`execute`] 里真的有那条分支 —— 两个清单漂移的后果是
/// **模型会去调一个必然失败的工具**（`TOOL_NOT_FOUND`），而界面上看不出为什么。
///
/// 当前三十五个：手机那四个只碰本机的（Skills 两个 + 长期记忆 + 提问）+ 桌面文件工具五件
/// （`list_dir` / `read_file` / `write_file` / `register_skill` / `delete_skill`，P4 第 40 步）
/// + 联网三件（`web_search` / `web_fetch` / `civitai_search`，见 [`crate::web_tools`] 与
/// [`crate::civitai`]，P4 第 64 步；它们本来就是 `BOTH` 档，只是**实现**在这一步才落地）
/// + comfy 查询三件（`comfy_get_run` / `comfy_list_workflows` / `comfy_get_status`）
/// + `comfy_load_workflow` / `comfy_submit` / `comfy_sync_history`
/// + 资料库两件（角色卡）+ **图像编辑三件**（`comfy_use_media` / `comfy_use_attachment` /
/// `comfy_make_mask`，见 [`crate::image_edit`]）
/// + **体检两件**（`comfy_check_inputs` / `comfy_fit_workflow`，见 [`crate::workflow_fit`]，
/// P4 第 62 步）
/// + **审核 + 摘拼两件**（`comfy_audit_workflow` / `comfy_prune_workflow`，见
/// [`crate::workflow_audit`] 与 [`crate::workflow_prune`]，P4 第 63 步）
/// + **定向查询**（`find_model`，见 [`crate::find_model`]，P4 第 64 步）
/// + **配方三件**（`comfy_learn_workflow` / `comfy_list_recipes` / `comfy_bind_recipe`，
/// 见 [`crate::workflow_recipe`]，P4 第 67 步；学与绑都与
/// `POST /api/ai/workflow-recipes*` 共用一份实现）。
pub const IMPLEMENTED: [&str; 35] = [
    "ask_user_question",
    "civitai_search",
    "comfy_audit_workflow",
    "comfy_bind_recipe",
    "comfy_check_inputs",
    "comfy_env_report",
    "comfy_find_workflow",
    "comfy_fit_workflow",
    "comfy_get_run",
    "comfy_get_status",
    "comfy_learn_workflow",
    "comfy_list_recipes",
    "comfy_list_workflows",
    "comfy_load_workflow",
    "comfy_make_mask",
    "comfy_prune_workflow",
    "comfy_submit",
    "comfy_sync_history",
    "comfy_use_attachment",
    "comfy_use_media",
    "danbooru_lookup",
    "delete_skill",
    "find_model",
    "list_dir",
    "list_skills",
    "load_skill",
    "read_file",
    "register_skill",
    "remember",
    "save_character_card",
    "search_character_cards",
    "search_prompts",
    "web_fetch",
    "web_search",
    "write_file",
];

/// 这个工具会不会写数据（系统提示里"（会写数据）"那个标记的真源）。
pub fn is_mutating(name: &str) -> bool {
    spec_of(name).map(|tool| tool.mutating).unwrap_or(false)
}

/// 出厂权限档（`ALLOW` / `ASK` / `DENY`）。
pub fn default_access(name: &str) -> &'static str {
    spec_of(name).map(|tool| tool.access).unwrap_or("ASK")
}

/// 在两个来源里找一条（找不到 → `None`，调用方按"没这个工具"处理）。
fn spec_of(name: &str) -> Option<ToolView<'static>> {
    crate::mobile_tools::MOBILE_TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .map(ToolView::from)
        .or_else(|| {
            crate::desktop_tools::DESKTOP_TOOLS
                .iter()
                .find(|tool| tool.name == name)
                .map(ToolView::from)
        })
}

/// 工具是否已实现（没实现的**不许下发**，也不许执行）。
///
/// ⚠ **不含运行时才拉到的 MCP 工具**：那一批的名单在 [`crate::mcp::McpRegistry`] 里，
/// 只有拿着 `AppState` 的调用方（执行器 / 界面端点）才问得到 —— 见
/// [`crate::mcp::McpRegistry::has`]。这里保持纯静态，是为了让"这一批真的实现了哪些"
/// 在源码里一眼可数（`IMPLEMENTED`）。
pub fn is_implemented(name: &str) -> bool {
    IMPLEMENTED.contains(&name)
}

/// 与 [`catalog`] 同一套判据，但**并上运行时拉到的 MCP 工具**（P4 第 45 步）。
///
/// 为什么要有它：`comfy-mcp` 的 39 个工具是运行时从子进程拉来的（清单在
/// [`crate::mcp::McpRegistry`] 里），静态的 [`catalog`] 看不见它们。桌面执行器与
/// `/api/ai/tools` 都要用"全量"（否则模型看不见能起实例 / 能跑工作流的那些工具，
/// 而 Kotlin 那边是看得见的 —— 两端行为会悄悄分叉）。
///
/// ⚠ 合并之后仍**按 name 升序**（系统提示里那两行与下发的 `tools[]` 顺序都从这里来）。
pub fn catalog_with(
    policy: &crate::policy::ToolPolicy,
    extra: &[ToolView<'static>],
) -> Vec<ToolView<'static>> {
    let mut out = catalog(policy);
    for view in extra {
        if policy.access_for(view.name, view.access) == crate::policy::ToolAccess::Deny {
            continue;
        }
        // 同名以静态清单为准（真出现同名说明两个来源漂移了，取先来的那个更稳）
        if out.iter().any(|tool| tool.name == view.name) {
            continue;
        }
        out.push(*view);
    }
    out.sort_by_key(|tool| tool.name);
    out
}

/// [`catalog_with`] 的 `ToolSpec` 版本（请求体里 `tools[]` 用的就是它）。
pub fn specs_with(
    policy: &crate::policy::ToolPolicy,
    extra: &[ToolView<'static>],
) -> Vec<crate::protocol::ToolSpec> {
    catalog_with(policy, extra).iter().map(spec_of_view).collect()
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
    policy: &crate::policy::ToolPolicy,
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
        // ---- 桌面文件工具五件（P4 第 40 步）--------------------------------
        "list_dir" => list_dir(policy, arguments),
        "read_file" => read_file(policy, arguments),
        "write_file" => write_file(policy, arguments),
        "register_skill" => register_skill(state, arguments),
        "delete_skill" => delete_skill(state, arguments),
        // ---- comfy 查询三件（P4 第 41/42 步）--------------------------------
        "comfy_get_run" => comfy_get_run(state, arguments),
        // 本机模型清单（`ComfyModels` 的 Rust 版）：**只报不改**，回答"盘上有 / 实例看不到"
        "comfy_env_report" => crate::comfy_models::tool_env_report(state, arguments).await,
        // 两段式找工作流：先搜库；库里 0 条就回头看本机（用户 bug ⑤）
        "comfy_find_workflow" => crate::workflow_search::tool_find_workflow(state, arguments),
        // ---- 体检两件（P4 第 62 步：用户建议 ①/⑤）----------------------------
        // 两件都是**只报不改**：`check_inputs` 列"还等着外部给的位置"（用户没给参考图时
        // 模型最容易硬提交），`fit_workflow` 把图里点名的模型与本机清单对一遍。
        // ⚠ `check_inputs` 不碰 `state`，要的是"本次带没带图片附件"——
        //   那个事实在 session 里（与喂给系统提示的是同一个值）。
        "comfy_check_inputs" => {
            crate::workflow_fit::tool_check_inputs(arguments, session.has_image_attachment)
        }
        "comfy_fit_workflow" => {
            crate::workflow_fit::tool_fit_workflow(state, arguments).await
        }
        "comfy_list_workflows" => comfy_list_workflows(state, arguments),
        "comfy_get_status" => comfy_get_status(state, arguments).await,
        "comfy_sync_history" => comfy_sync_history(state).await,
        "comfy_load_workflow" => comfy_load_workflow(state, policy, arguments).await,
        // ---- 审核 + 摘拼两件（P4 第 63 步：用户 2026-09-22 的"审核本地工作流"）---------
        // `audit` 是**只报事实**的确定性检查（"必跑不了"与"不推荐"分得很清）；
        // `prune` 是它的对偶 —— 分析与摘除都走**同一套可达性判据**（从输出节点反向走不到
        // 的才算无关分支），所以"分析说要摘的那几条"拿去执行一定是同一片。
        "comfy_audit_workflow" => {
            crate::workflow_audit::tool_audit_workflow(state, arguments).await
        }
        "comfy_prune_workflow" => {
            crate::workflow_prune::tool_prune_workflow(state, arguments).await
        }
        // ---- 配方三件（P4 第 67 步：用户"新模型该怎么接上已有骨架"）----------------
        // 学（归纳 + 落库）与绑（只报不改地算出该改哪几个 overrides 键）都走
        // `workflow_recipe` 里那两个函数 —— 与 `POST /api/ai/workflow-recipes*`
        // **共用一份实现**：分头写必然漂移，而漂移的表现是"AI 学出来的配方和界面看到的不一样"（不报错）。
        "comfy_learn_workflow" => {
            crate::workflow_recipe::tool_learn_workflow(state, policy, arguments).await
        }
        "comfy_list_recipes" => crate::workflow_recipe::tool_list_recipes(state, arguments),
        "comfy_bind_recipe" => crate::workflow_recipe::tool_bind_recipe(state, arguments).await,
        // ---- 提交（P4 第 48 步：**R10 的落点**）------------------------------
        // 「审核 → 审批 → 提交 → 自己等 → captureRun 入库 → mediaIds」这条链：
        // 审批由执行器（`default_access = ASK`）负责，其余都在 `workflow_submit` 里。
        "comfy_submit" => crate::workflow_submit::tool_submit(state, policy, arguments).await,
        // ---- 图像编辑三件（图像编辑那一批：投放 + 遮罩）----------------------
        // 两阶段编辑（"先出人物、再换上去" / "换背景保人物"）的地基：ComfyUI 的
        // LoadImage / LoadImageMask **只能读它自己的 input 目录**，而画廊产物与用户附件
        // 都不在那里 —— 这两件工具是唯一的投放通道，`comfy_make_mask` 则解决
        // "手机 / 平板上画不出遮罩"这件事（与 Kotlin `AiImageEdit` 逐条对照）。
        "comfy_use_media" => crate::image_edit::tool_use_media(state, arguments),
        "comfy_use_attachment" => crate::image_edit::tool_use_attachment(state, arguments),
        "comfy_make_mask" => crate::image_edit::tool_make_mask(state, arguments),
        // ---- 资料库两件（P4 第 46 步：角色卡）--------------------------------
        // O7/O8 的落点：写角色之前**先查本机**（库里那张卡 / 那条提示词的 tag 栈），
        // 而不是凭印象编 —— 现场教训是"把 shinku 当成 iroi 写了两版废图"。
        "search_character_cards" => crate::character_cards::tool_search(state, arguments),
        "save_character_card" => crate::character_cards::tool_save(state, arguments, session),
        // `search_prompts`：同一个落点的另一半 —— 库里可能已经有那个角色的完整 tag 栈
        "search_prompts" => search_prompts(state, arguments),
        // ---- 联网三件 + 定向查询（P4 第 64 步：用户 2026-09-20 的"只用通用方案"）------
        // 这个类别是**本项目里唯一会把数据送出本机**的那一类（`ai_settings::category_of`
        // 回 "web"），所以三件事绑在一起：出厂 `ASK`（用户点一下才发出去）、
        // 结果前面套**数据区标记**（RSK-004）、以及共用同一份
        // `MAX_WEB_QUERIES_PER_RUN` 预算（执行器按类别判，见 `executor::handle_call`）。
        "web_search" => crate::web_tools::tool_search(state, arguments).await,
        "web_fetch" => crate::web_tools::tool_fetch(state, arguments).await,
        "civitai_search" => crate::civitai::tool_search(state, arguments).await,
        "find_model" => crate::find_model::tool_find_model(state, arguments).await,
        // ---- danbooru（联网，唯一会把数据送出本机的工具）----------------------
        // 查完必须把"这一轮真的查过哪个 tag"记进 session：`save_character_card`
        // 靠它决定写不写 `tagsVerifiedAt`（**核对时间是事实，不是模型的自称**）。
        "danbooru_lookup" => {
            let outcome = crate::danbooru::tool_lookup(arguments).await;
            if let Ok(outcome) = outcome.as_ref() {
                session.mark_danbooru_verified(arguments.get("query").and_then(Value::as_str).unwrap_or(""));
                if let Some(tag) = outcome
                    .payload
                    .get("character")
                    .and_then(|c| c.get("tag"))
                    .and_then(Value::as_str)
                {
                    session.mark_danbooru_verified(tag);
                }
            }
            outcome
        }
        // ---- 官方 comfy-mcp 的 39 个工具（P4 第 45 步）------------------------
        //
        // 名字是**运行时**拉来的（`tools/list`），所以这里用"注册表里有没有"来兜底，
        // 而不是往 `IMPLEMENTED` 里塞 39 个字符串 —— 那会变成两份会漂移的清单。
        _ if state.mcp.has(name) => state.mcp.call(&state.project_root(), name, arguments).await,
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

// ---------------------------------------------------------------------------
//  参数取值与"JVM 口径"小工具（Kotlin `ToolRegistry.kt` 末尾那一节 + 三个出网工具
//  共用的口径）
//
//  ⚠ 这些是**三件出网工具共用的一份**：`web_search` / `web_fetch` / `civitai_search` /
//    `find_model` 的编码与空白处理必须逐字一致 —— 各写一份的下场是"某一条工具的
//    查询词悄悄编错，而它不报错、只是搜不到东西"。
// ---------------------------------------------------------------------------

/// `JsonObject.str`（Kotlin）的对应物：**任意 JSON 基本类型**取字面量（数字 / 布尔也算），
/// 空白算"没给"，对象 / 数组 / null 一律 `None`。
///
/// 为什么与 [`string_arg`] 并存：那个是"参数必须是字符串"（缺了就报错），
/// 这个是"参数可以宽松取，取不到算没给"（`args.int("limit") ?: 8` 那种写法）。
pub(crate) fn str_arg(arguments: &Value, key: &str) -> Option<String> {
    let raw = match arguments.get(key)? {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    if raw.trim().is_empty() {
        None
    } else {
        Some(raw)
    }
}

/// 布尔参数（Kotlin `JsonObject.bool`：`content.toBooleanStrictOrNull()` 的对应物）——
/// JSON 布尔与**恰好 `"true"` / `"false"`** 的字符串都认，其余一律 `None`。
pub(crate) fn bool_arg(arguments: &Value, key: &str) -> Option<bool> {
    let raw = arguments.get(key)?;
    if let Some(value) = raw.as_bool() {
        return Some(value);
    }
    match raw.as_str()? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// `URLEncoder.encode(value, UTF_8)` 的逐字对应物（**Java 口径**，不是 RFC 3986）：
/// 保留字母数字与 `-_.*`，**空格变 `+`**，其余按 UTF-8 字节百分号编码（大写十六进制）。
///
/// ⚠ 与 [`crate::danbooru::encode`] 的差别是**有意的**：Kotlin 的 `DanbooruSearch.encode`
/// 在 `URLEncoder` 之后多做了 `.replace("+", "%20")`，而 Keenable / Civitai / FindModel
/// 的 Kotlin 实现**没有**那一步 —— 所以这里必须留 `+`，改了就是与 Kotlin 不一致。
pub(crate) fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '*') {
            out.push(ch);
        } else if ch == ' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Kotlin `Regex("\\s+").replace(text, " ")` + `String.trim()`。
///
/// ⚠ 空白是 **ASCII 口径**（Java 的 `\s` 默认只认 `[ \t\n\x0B\f\r]`，`String.trim()`
/// 也只去 `<= U+0020`）：全角空格 / NBSP 在 Kotlin 那边**不算空白**，这里也不许算 ——
/// 否则简介会被多压掉几个字符，模型看到的正文就与 Kotlin 不一样。
pub(crate) fn collapse_spaces(text: &str) -> String {
    let mut out = String::new();
    let mut in_space = false;
    for ch in text.chars() {
        if matches!(ch, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r') {
            in_space = true;
            continue;
        }
        if in_space && !out.is_empty() {
            out.push(' ');
        }
        in_space = false;
        out.push(ch);
    }
    out
}

/// Java `String.trim()`（只去 `<= U+0020` 的字符，不是 Unicode 空白）。
pub(crate) fn java_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c <= '\u{20}')
}

/// 环境变量取值：**空白当没配**，回落到默认值（Kotlin 那几处
/// `System.getenv("X")?.takeIf { it.isNotBlank() } ?: DEFAULT` 的对应物）。
pub(crate) fn env_or(name: &str, fallback: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// Kotlin `String.length` 数的是 **UTF-16 单元**（中文算 1、emoji 算 2）。
///
/// 联网工具那两处按"字符数"做的预算（`CONTEXT_MAX_CHARS` / `take(n)`）必须与它同口径，
/// 否则"要不要留下这一条""截到多少字"会与 Kotlin 差几个字符 —— 而那种差别不报错。
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Kotlin `String.take(n)` 的对应物（按 UTF-16 单元切，**不切碎代理对**）。
pub fn take_utf16(text: &str, limit: usize) -> String {
    if utf16_len(text) <= limit {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let width = ch.len_utf16();
        if used + width > limit {
            break;
        }
        used += width;
        out.push(ch);
    }
    out
}

/// 联网工具结果前面那行**数据区出处标记**（Kotlin `untrustedBanner`）。
///
/// 模板随 run-kit 原样下发（R7：两端不许各写一份），这里只是把 `{kind}` / `{query}`
/// 替换掉。第二行**只在有查询词时**带 —— 差别进了模型看到的文本，所以照抄。
pub fn untrusted_banner(kind: &str, query: Option<&str>) -> String {
    let mut out = crate::mobile_tools::UNTRUSTED_BANNER_TEMPLATE.replace("{kind}", kind);
    if let Some(query) = query.filter(|q| !q.trim().is_empty()) {
        out.push_str(&crate::mobile_tools::UNTRUSTED_BANNER_QUERY_LINE.replace("{query}", query));
    }
    out
}

/// 一次出网失败的**语义分类**（Kotlin 那边靠 Java 异常类型分的三档）。
///
/// 为什么不能只看 `is_timeout()`：连接超时（5 秒连不上）与读超时（连上了但没回完）
/// 在用户那边是**两条不同的排查路**（前者多半是代理 / 出网，后者是对方慢），
/// 而两者混成一句"超时（25 秒）"时用户按"稍后再试"永远好不了 —— 这是 Kotlin 侧
/// 明确修过的一个"说谎的提示"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutboundError {
    /// 连接阶段就没上去（`HttpConnectTimeoutException`）
    ConnectTimeout,
    /// 连上了但整次请求超时（`HttpTimeoutException`）
    ReadTimeout,
    /// 被立刻拒 / DNS 失败 / 其它（`ConnectException` 那一档）
    Other,
}

/// 见 [`OutboundError`]。判据是 reqwest 的两条：`is_connect()` 与 `is_timeout()`。
pub(crate) fn outbound_error_kind(error: &reqwest::Error) -> OutboundError {
    if error.is_timeout() && error.is_connect() {
        OutboundError::ConnectTimeout
    } else if error.is_timeout() {
        OutboundError::ReadTimeout
    } else {
        OutboundError::Other
    }
}

/// Kotlin 错误文案里那个 `${e::class.simpleName}`。
///
/// Rust 没有 Java 异常类名，这里给一个**同名等价的分类**：文案一个字符都不用改
/// （`连不上搜索服务（ConnectException）；…`），改的只是这个名字从哪来 —— 本机
/// civitai / HF 直连不通时 Java 侧报的正是 `ConnectException`。
pub(crate) fn java_error_name(error: &reqwest::Error) -> &'static str {
    if error.is_connect() {
        "ConnectException"
    } else if error.is_timeout() {
        "HttpTimeoutException"
    } else if error.is_body() || error.is_decode() {
        "IOException"
    } else {
        "IOException"
    }
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

// ---------------------------------------------------------------------------
//  桌面文件工具五件（P4 第 40 步）
// ---------------------------------------------------------------------------

/// 整数参数（Kotlin `JsonObject.int`：数字与**数字字符串**都认，认不出 → `None`）。
pub(crate) fn int_arg(arguments: &Value, key: &str) -> Option<i64> {
    let raw = arguments.get(key)?;
    if let Some(n) = raw.as_i64() {
        return Some(n);
    }
    raw.as_str().and_then(|s| s.trim().parse::<i64>().ok())
}

/// `list_dir`：列目录（默认 = 写白名单第一个，出厂是 `<根>\comfyui`）。
///
/// 两条与 Kotlin 一致的行为：
/// - 默认路径取自 **`write_roots` 的第一个**（不是读白名单）—— 那是"用户最可能想看的那个目录"；
/// - 超过 200 条**明说截断**（静默截断会让模型以为目录里就这么多东西）。
///
/// ⚠ 一处**有意**与 Kotlin 不同：条目按名字排序。Java 的 `Files.walk` 走的是
/// `DirectoryStream` 的**文件系统顺序**（同一目录两次跑可能不一样），而这份输出会进上下文，
/// 顺序飘会让模型的复述与用例都变得不稳；排序不改内容、只改次序，所以这里选确定性。
fn list_dir(
    policy: &crate::policy::ToolPolicy,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let raw = arguments
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            policy.write_roots.first().map(|p| p.display().to_string()).unwrap_or_default()
        });
    let dir = policy.resolve_read(&raw)?;
    if !dir.is_dir() {
        return Err(ToolFailure::new("NOT_A_DIRECTORY", format!("不是目录：{}", dir.display())));
    }
    let depth = int_arg(arguments, "depth").unwrap_or(1).clamp(1, 2);
    let max_entries = 200usize;
    let mut lines: Vec<String> = Vec::new();
    let mut truncated = false;
    for path in walk_limited(&dir, depth as usize) {
        if path == dir {
            continue;
        }
        if lines.len() >= max_entries {
            truncated = true;
            break;
        }
        let rel = path
            .strip_prefix(&dir)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| path.display().to_string());
        if path.is_dir() {
            lines.push(format!("dir  {rel}"));
        } else {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            lines.push(format!("file {rel} {size}B"));
        }
    }
    let mut text = dir.display().to_string();
    text.push('\n');
    let listing = lines.join("\n");
    text.push_str(if lines.is_empty() { "(空目录)" } else { listing.as_str() });
    if truncated {
        text.push_str(&format!("\n…（超过 {max_entries} 条已截断）"));
    }
    Ok(ToolOutcome::new(text, Value::Null))
}

/// 有界遍历（Kotlin `Files.walk(dir, depth)` 的对应物）：**含** `dir` 自身，
/// 每个目录内的条目按名字排序（见 [`list_dir`] 里那条说明）。
fn walk_limited(dir: &std::path::Path, max_depth: usize) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![dir.to_path_buf()];
    let mut frontier: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];
    while let Some((current, level)) = frontier.pop() {
        if level >= max_depth {
            continue;
        }
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(&current) {
            Ok(iter) => iter.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
            // 读不了的目录（权限 / 竞态）**跳过**，不让整个 list_dir 失败
            Err(_) => continue,
        };
        entries.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
        for path in entries {
            out.push(path.clone());
            if path.is_dir() {
                frontier.push((path, level + 1));
            }
        }
    }
    out
}

/// `read_file`：分段读一个文本文件，**切点落在字符边界上**。
///
/// ⚠ 描述里写着"返回里会给出 nextOffset"，但 Kotlin 的实现**从没给过这个键**
/// （`ToolOutput(text)` 的 meta 是 null）—— 真正给模型的是正文末尾那句
/// "接着读请再调一次 read_file，path 不变、offset=N"。这里保持逐字一致，
/// **不自己发明一个 `nextOffset`**：多一个键就多一处两端不一致。
fn read_file(
    policy: &crate::policy::ToolPolicy,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let raw = string_arg(arguments, "path")?;
    let file = policy.resolve_read(&raw)?;
    if !file.is_file() {
        return Err(ToolFailure::new("NOT_A_FILE", format!("不是文件：{}", file.display())));
    }
    let limit = int_arg(arguments, "maxBytes")
        .unwrap_or(crate::policy::DEFAULT_MAX_READ_BYTES)
        .clamp(1, crate::policy::DEFAULT_MAX_READ_BYTES);
    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    let start = int_arg(arguments, "offset").unwrap_or(0).clamp(0, size as i64) as u64;

    // 只读这一窗（不把整个文件读进内存：`maxBytes` 2MB，而文件可能是几百 MB 的视频）
    let window = read_window(&file, start, limit as usize)?;
    let from = snap_to_char_start(&window);
    let to = from + complete_utf8_prefix(&window, from);
    let text = String::from_utf8_lossy(&window[from..to]).to_string();
    let read_start = start as i64 + from as i64;
    let end = start as i64 + to as i64;
    let note = if size as i64 > end {
        format!(
            "\n…（文件共 {size} 字节，本次是 [{read_start}, {end})；接着读请再调一次 read_file，\
             path 不变、offset={end}）"
        )
    } else {
        String::new()
    };
    Ok(ToolOutcome::new(format!("{}\n{text}{note}", file.display()), Value::Null))
}

/// 读文件的 `[start, start+limit)` 这一段（短读是正常的：文件可能就到头了）。
fn read_window(file: &std::path::Path, start: u64, limit: usize) -> Result<Vec<u8>, ToolFailure> {
    use std::io::{Read, Seek, SeekFrom};
    let mut handle = std::fs::File::open(file)
        .map_err(|e| ToolFailure::new("IO_ERROR", format!("打不开文件（{}）：{e}", file.display())))?;
    if start > 0 {
        handle
            .seek(SeekFrom::Start(start))
            .map_err(|e| ToolFailure::new("IO_ERROR", format!("定位失败（{start}）：{e}")))?;
    }
    let mut buffer = Vec::with_capacity(limit.min(64 * 1024));
    let mut chunk = handle.take(limit as u64);
    chunk
        .read_to_end(&mut buffer)
        .map_err(|e| ToolFailure::new("IO_ERROR", format!("读文件失败：{e}")))?;
    Ok(buffer)
}

/// 起始点落在**字符边界**上（Kotlin `snapToCharStart`）：跳过多达 3 个续字节 `10xxxxxx`。
fn snap_to_char_start(bytes: &[u8]) -> usize {
    let mut i = 0;
    while i < bytes.len() && i < 4 && (bytes[i] & 0xC0) == 0x80 {
        i += 1;
    }
    i
}

/// 从 `from` 起、**完整** UTF-8 字符覆盖的字节数（Kotlin `completeUtf8Prefix`）。
///
/// 这是"按字节分段读中文"的命门：切点大概率落在一个汉字中间，直接解码会在页尾留一个
/// U+FFFD（乱码），模型照着改回去就把文件写坏了。退回上一个字符边界即可，**一个字都不丢**。
fn complete_utf8_prefix(bytes: &[u8], from: usize) -> usize {
    if from >= bytes.len() {
        return 0;
    }
    let mut i = bytes.len();
    let mut back = 0;
    while i > from && back < 4 {
        if (bytes[i - 1] & 0xC0) != 0x80 {
            break;
        }
        i -= 1;
        back += 1;
    }
    if i == from {
        // 整段都是续字节：交给解码器兜底（Kotlin 同）
        return bytes.len() - from;
    }
    let lead = bytes[i - 1];
    let need = if lead & 0x80 == 0x00 {
        1
    } else if lead & 0xE0 == 0xC0 {
        2
    } else if lead & 0xF0 == 0xE0 {
        3
    } else if lead & 0xF8 == 0xF0 {
        4
    } else {
        1
    };
    let have = bytes.len() - (i - 1);
    if have >= need {
        bytes.len() - from
    } else {
        (i - 1) - from
    }
}

/// `write_file`：**默认只允许写 ComfyUI 目录**（`comfyui/`）。
///
/// 三条与 Kotlin 逐字一致：超限报 `TOO_LARGE` 并**告诉模型怎么分段写**（不是干巴巴一句失败）、
/// 路径过 [`crate::policy::ToolPolicy::resolve_write`]、`mode="append"` 才追加。
fn write_file(
    policy: &crate::policy::ToolPolicy,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let raw = string_arg(arguments, "path")?;
    let content = string_arg(arguments, "content")?;
    let bytes = content.as_bytes();
    if bytes.len() as i64 > crate::policy::DEFAULT_MAX_WRITE_BYTES {
        return Err(ToolFailure::new(
            "TOO_LARGE",
            format!(
                "内容太大（{} > {} 字节）：拆成多次写（第一次 overwrite、之后用 mode=\"append\"），\
                 别因为一次写不下就放弃",
                bytes.len(),
                crate::policy::DEFAULT_MAX_WRITE_BYTES
            ),
        ));
    }
    let target = policy.resolve_write(&raw)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ToolFailure::new("IO_ERROR", format!("建不了目录（{}）：{e}", parent.display())))?;
    }
    let append = arguments.get("mode").and_then(Value::as_str) == Some("append");
    let result = if append {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&target)
            .and_then(|mut f| f.write_all(bytes))
    } else {
        std::fs::write(&target, bytes)
    };
    result.map_err(|e| ToolFailure::new("IO_ERROR", format!("写不了文件（{}）：{e}", target.display())))?;
    Ok(ToolOutcome::new(
        format!(
            "已写入 {}（{} 字节，{}）。",
            target.display(),
            bytes.len(),
            if append { "追加" } else { "覆盖" }
        ),
        json!({ "path": target.display().to_string(), "bytes": bytes.len() }),
    ))
}

/// `register_skill`：把一个新 Skill 写进用户根（磁盘是真源，写完立刻生效）。
fn register_skill(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let name = string_arg(arguments, "name")?;
    let description = string_arg(arguments, "description")?;
    let content = string_arg(arguments, "content")?;
    let when_to_use = arguments.get("whenToUse").and_then(Value::as_str);
    let dto = crate::skills::save(state, &name, &description, when_to_use, &content)
        .map_err(skill_failure)?;
    let payload = serde_json::to_value(&dto).unwrap_or(Value::Null);
    Ok(ToolOutcome::new(
        format!(
            "已注册 skill「{}」（{} 字节，digest {}）。\
             下一次回复即可用 load_skill 加载它；用户也能在右侧栏看到并删除。",
            dto.name,
            dto.size_bytes,
            dto.digest.clone().unwrap_or_default()
        ),
        payload,
    ))
}

/// `delete_skill`：只允许删**用户来源**的（内置受保护，AIH-037）。
fn delete_skill(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let name = string_arg(arguments, "name")?;
    let deleted = crate::skills::delete(state, &name).map_err(skill_failure)?;
    if !deleted {
        return Err(ToolFailure::new(
            "SKILL_NOT_FOUND",
            format!("没有名为「{name}」的 skill"),
        ));
    }
    Ok(ToolOutcome::new(
        format!("已删除 skill「{name}」。"),
        json!({ "name": name, "deleted": true }),
    ))
}

/// `SkillFailure{code, message}` → 工具失败（**码原样透传**：`SKILL_READONLY` /
/// `INVALID_ARGUMENT` 各有各的下一步，改写成 `TOOL_ERROR` 会让模型分不清该怎么办）。
fn skill_failure(failure: crate::skills::SkillFailure) -> ToolFailure {
    ToolFailure::new(failure.code, failure.message)
}

/// `comfy_get_run`：按 runKey 查一次已捕获运行（Kotlin `comfyFindRun`）。
///
/// 两个入口都认（Kotlin 那边为这个专门加了 `findRunByPromptId`）：UUID 形式的 `run_key`，
/// 以及**数字**的 `capture_runs.prompt_id`。只认 UUID 的话，模型拿 `comfy_submit` 结果里的
/// 数字来查就会"刚提交完却说没这次运行"（真实对话里发生过）。
fn comfy_get_run(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let key = string_arg(arguments, "runKey")?;
    let trimmed = key.trim();
    let found = crate::capture::find_run(state, trimmed).map_err(capture_failure)?;
    let found = match found {
        Some(info) => Some(info),
        None => match trimmed.parse::<i64>() {
            Ok(prompt_id) => {
                crate::capture::find_run_by_prompt_id(state, prompt_id).map_err(capture_failure)?
            }
            Err(_) => None,
        },
    };
    let Some(info) = found else {
        return Err(ToolFailure::new(
            "NOT_FOUND",
            format!(
                "没有 runKey={trimmed} 的捕获记录（可能还没被轮询到，或不属于本机）。\
                 注意 comfy_submit 的结果里 `runKey` 是 ComfyUI 的 UUID、\
                 `capturedPromptId` 是捕获记录的编号，两者都能查。"
            ),
        ));
    };
    let payload = info.to_json();
    Ok(ToolOutcome::new(payload.to_string(), payload))
}

/// `search_prompts`：搜**本机的提示词库**，或按 id 取一条完整的（Kotlin `search_prompts`）。
///
/// 为什么非得有它（O7 的现场）：AI 以前**没有"搜我自己的库"这条路**，只能凭印象编 tag ——
/// 于是把参考图里的 `shinku` 当成 `iroi` 写了两版废图，而库里 id 261/310 **就有**
/// `shinku_(nte), neverness_to_everness, pink streaks, purple horns…` 的完整 tag 栈。
///
/// ⚠ 查询走 [`crate::routes::query_prompts`]（**与 `/api/prompts` 同一个实现**）：
/// 各写一份 SQL 的后果是"界面上搜得到、AI 说没有"，而那种不一致不会有任何用例变红。
fn search_prompts(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let id = int_arg(arguments, "id");
    let limit = int_arg(arguments, "limit").unwrap_or(5).clamp(1, MAX_LIBRARY_RESULTS);
    // Kotlin：`includePrompt` 缺席或不是 "false" 都算 true（只显式写 false 才省掉正文）
    let include_prompt = arguments.get("includePrompt").and_then(Value::as_str) != Some("false");
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_string);

    let items = if let Some(id) = id {
        // 按 id 取那一条（与 `/api/prompts/{id}` 同一个取法：标签 + 产物数一起补齐）
        vec![crate::routes::get_prompt_dto(state, id).map_err(|_| {
            ToolFailure::new(
                "NOT_FOUND",
                format!("库里没有 id={id} 的提示词；用 query 搜一下角色名或 tag 片段"),
            )
        })?]
    } else {
        crate::routes::query_prompts(
            state,
            &crate::routes::PromptQuery {
                q: query.clone(),
                kind: None,
                favorite: None,
                sort: Some("newest".to_string()),
                page: Some(1),
                size: Some(limit),
            },
        )
        .map_err(library_failure)?
        .items
    };

    if items.is_empty() {
        return Err(ToolFailure::new(
            "NOT_FOUND",
            format!(
                "库里没有匹配的提示词（关键字：{}）。换个词再搜（角色名 / 作品名 / 一个 tag），\
                 或者换 `comfy_list_workflows` 找本机的工作流文件 —— **不要**因为库里没有就自己编 tag。",
                query.as_deref().unwrap_or("（空）")
            ),
        ));
    }

    let mut text = String::new();
    text.push_str(&format!(
        "提示词库：命中 {} 条{}\n",
        items.len(),
        id.map(|i| format!("（id={i}）")).unwrap_or_default()
    ));
    for prompt in items.iter() {
        text.push('\n');
        text.push_str(&format!(
            "── [{}] {}\n",
            prompt.id,
            if prompt.title.is_empty() { "(无标题)" } else { prompt.title.as_str() }
        ));
        text.push_str(&format!(
            "   {} · 来源 {} · 产物 {} 张 · {}\n",
            prompt.kind,
            prompt.source.as_deref().unwrap_or("?"),
            prompt.media_count,
            prompt.created_at.as_deref().unwrap_or("?")
        ));
        let loras = if prompt.loras.is_empty() {
            "（无）".to_string()
        } else {
            prompt
                .loras
                .iter()
                .map(|l| format!("{}@{}", l.name, l.weight))
                .collect::<Vec<_>>()
                .join("、")
        };
        text.push_str(&format!("   LoRA: {loras}\n"));
        let size = match (prompt.width, prompt.height) {
            (Some(w), Some(h)) => format!("{w}x{h} "),
            _ => String::new(),
        };
        text.push_str(&format!(
            "   参数: {size}steps={} cfg={} sampler={} seed={} 底模={}\n",
            prompt.steps.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
            prompt.cfg_scale.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
            prompt.sampler.as_deref().unwrap_or("?"),
            prompt.seed.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
            prompt.checkpoint.as_deref().unwrap_or("?")
        ));
        if !prompt.tags.is_empty() {
            let names: Vec<&str> = prompt.tags.iter().map(|t| t.name.as_str()).collect();
            text.push_str(&format!("   tag: {}\n", names.join("、")));
        }
        if include_prompt {
            text.push_str(&format!("   正面: {}\n", clip_text(&prompt.positive_prompt, MAX_PROMPT_CHARS)));
            if let Some(negative) = prompt.negative_prompt.as_deref().filter(|n| !n.trim().is_empty()) {
                text.push_str(&format!("   负面: {}\n", clip_text(negative, MAX_PROMPT_CHARS / 2)));
            }
        }
    }
    text.push('\n');
    text.push_str(
        "用法：照抄角色那段 tag 栈（或整条正向）；要按这条工作流出图就用 comfy_submit(promptId=<上面的 id>)。",
    );

    let payload = json!({
        "count": items.len(),
        "items": items
            .iter()
            .map(|prompt| {
                let mut out = serde_json::Map::new();
                out.insert("id".into(), json!(prompt.id));
                out.insert("title".into(), json!(prompt.title));
                out.insert("kind".into(), json!(prompt.kind));
                out.insert("source".into(), json!(prompt.source.clone().unwrap_or_default()));
                out.insert("mediaCount".into(), json!(prompt.media_count));
                out.insert("checkpoint".into(), json!(prompt.checkpoint.clone().unwrap_or_default()));
                out.insert("loras".into(), json!(prompt.loras.iter().map(|l| l.name.clone()).collect::<Vec<_>>()));
                out.insert("tags".into(), json!(prompt.tags.iter().map(|t| t.name.clone()).collect::<Vec<_>>()));
                if include_prompt {
                    out.insert("positivePrompt".into(), json!(prompt.positive_prompt));
                    out.insert("negativePrompt".into(), json!(prompt.negative_prompt.clone().unwrap_or_default()));
                }
                Value::Object(out)
            })
            .collect::<Vec<_>>(),
    });
    Ok(ToolOutcome::new(text, payload))
}

/// 单条提示词（按 id 取，含标签与产物数）—— 与 `GET /api/prompts/{id}` 同一条路。


///
/// 与 `GET /api/capture/workflows` **同一个实现**（[`crate::comfy_dirs::workflows_json`]）——
/// 三份目录来源、`file:<sha256>` 那把幂等钥匙、`inLibrary` 判定全都不另写一份。
/// 各写一份的后果是"界面里列得出来、AI 却说没有"（用户 bug ⑤ 的复发形态）。
fn comfy_list_workflows(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let limit = int_arg(arguments, "limit").unwrap_or(20).clamp(1, 100) as usize;
    let json = crate::comfy_dirs::workflows_json(
        state,
        crate::comfy_dirs::WorkflowQuery { q: query, limit: Some(limit) },
    )
    .map_err(capture_failure)?;
    Ok(ToolOutcome::new(json.to_string(), json))
}

/// `comfy_get_status`：ComfyUI 连通性 + 队列 + 最近捕获记录（Kotlin `comfyStatus`）。
///
/// ★ **与 Kotlin 的一处有意不同**：`recentLimit` 在 Kotlin 那边**读了但没用**
/// （`comfyStatus()` 永远给 `recentRuns(20)`，`limit` 只影响末尾那句"已省略"的提示）。
/// 工具描述写的是"默认 5，最多 20"，所以这里**按描述真的裁**（正文与 meta 一起裁，
/// 裁掉多少也说出来）—— 描述与行为不一致比"和 Kotlin 差几个元素"更坏，
/// 模型会照着描述去数。端点 `/api/capture/status` 仍是 20 条（那是与 Kotlin 的**形状**对齐，
/// 别一起裁）。
async fn comfy_get_status(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let limit = int_arg(arguments, "recentLimit").unwrap_or(5).clamp(0, 20);
    let include_recent = arguments
        .get("includeRecent")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut status = crate::capture::status_json(state).await.map_err(capture_failure)?;
    // 最近提交过的任务（Kotlin `comfyStatus` = `CaptureStatus` + `submitter.submissions(5)`）。
    //
    // ⚠ **这里以前恒给空数组**（第 42 步写的注释是"Rust 侧 `comfy_submit` 还没搬 → 此刻确实一个都没有"）
    //   —— 第 48 步 `comfy_submit` 搬来之后 `state.submissions` 真的有内容，那句注释就变成了**假话**：
    //   模型刚提交完就来查状态，看到 `[]` 会以为自己**根本没提交过**（正是 Kotlin 在那行旁边写下的
    //   "模型查状态时就能看到自己刚提交的那个跑到哪了"）。取最近 5 条，与 Kotlin 同。
    if let Some(obj) = status.as_object_mut() {
        obj.insert("submissions".into(), Value::Array(state.submissions.list(5)));
    }
    // **抠图模型在不在**（2026-09-27）：`easy imageRemBg` 只读 `<ComfyUI 根>\models\rembg`，
    // 而文件不在时它会自己去 HuggingFace 静默下载（表现是"点了按钮没反应"）。
    // 这里一次报清，让模型"先查再跑、缺了就指引用户下"—— 免得它去猜路径或者干脆默默降级。
    // ⚠ 这是**加在工具返回里**（不是加在 `/api/capture/status` 那个端点里）：那个端点的键集合
    //   是界面契约（有用例钉着精确键集合），加一个键就会红。
    if let Ok(config) = crate::capture::capture_config(state) {
        let roots = crate::comfy_dirs::comfy_homes(&state.project_root(), config.output_dir.as_deref());
        if let Some(obj) = status.as_object_mut() {
            obj.insert("mattingModels".into(), crate::image_edit::matting_models(&roots));
        }
    }

    let mut omitted = 0usize;
    if include_recent && limit > 0 {
        if let Some(list) = status.get_mut("recent").and_then(Value::as_array_mut) {
            omitted = list.len().saturating_sub(limit as usize);
            list.truncate(limit as usize);
        }
    } else if let Some(obj) = status.as_object_mut() {
        obj.remove("recent");
    }
    let text = if omitted > 0 {
        format!("{status}\n（最近捕获记录只列了 {limit} 条，还有 {omitted} 条没列；要看全部就用 recentLimit 放大它，上限 20）")
    } else if !include_recent || limit == 0 {
        format!("{status}\n（最近捕获记录已省略）")
    } else {
        status.to_string()
    };
    Ok(ToolOutcome::new(text, status))
}

/// `comfy_sync_history`：立刻同步一次 `/history`（**会写库**：建提示词 + 产物入库）。
///
/// 与 `POST /api/capture/poll` **同一个实现**（[`crate::capture_ingest::poll_once`]）——
/// 两处各写一份的后果是"界面上收不到的、AI 说收到了"（或反过来）。
/// 它是 `mutating = true` + 出厂档 `ASK`：动手前必须用户点一次批准（R8 的闸门在调用方）。
async fn comfy_sync_history(state: &AppState) -> Result<ToolOutcome, ToolFailure> {
    let config = crate::capture::capture_config(state).map_err(capture_failure)?;
    let result = crate::capture_ingest::poll_once(state, &config).await;
    let text = result.to_string();
    Ok(ToolOutcome::new(text, result))
}

/// `comfy_load_workflow`：把一份**本机工作流文件**读进库，换成能提交的 `promptId`（用户 bug ③）。
///
/// 路径走**读白名单**（[`crate::policy::ToolPolicy::resolve_read`]）：本机 ComfyUI 的目录
/// 已经被自动放行（用户 bug ④），用户的工作流一般就躺在它下面的 `user\<用户>\workflows` 里。
///
/// ⚠ `emulateFrontend` **收下但不改变行为**（本后端还没实现"模拟用户行为"）——
/// 这正是 `workflow_load` 会在结果里补一句 warning 的原因：工具描述是从 Kotlin 逐字搬的、
/// 里面写着这个能力，**不说明就等于默认它做了**（那会让用户以为前端节点已经被接好）。
async fn comfy_load_workflow(
    state: &AppState,
    policy: &crate::policy::ToolPolicy,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let raw = string_arg(arguments, "path")?;
    let file = policy.resolve_read(&raw)?;
    let title = arguments
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let include_graph = arguments.get("includeGraph").and_then(Value::as_bool).unwrap_or(false);
    let tolerate = arguments
        .get("tolerateUnsupported")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let emulate = arguments.get("emulateFrontend").and_then(Value::as_bool).unwrap_or(true);
    let json = crate::workflow_load::load_from_file(
        state,
        &file,
        title,
        include_graph,
        tolerate,
        emulate,
    )
    .await
    .map_err(|failure| ToolFailure::new(&failure.code, failure.message))?;
    Ok(ToolOutcome::new(json.to_string(), json))
}

/// `ApiError` → 工具失败（只让**这一次调用**失败，不打断整个 Run）。
fn capture_failure(error: crate::routes::ApiError) -> ToolFailure {
    ToolFailure::new("TOOL_ERROR", format!("本机数据查询出错：{}", error.human()))
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
        // 联网三件 + 定向查询（P4 第 64 步）：**应当**出现在清单里 ——
        // 系统提示第 17 条点名的那四个工具到这一步才真的下发给模型（在此之前那一节指向
        // 的是模型看不见的工具，等于一句空话）。
        for expected in ["web_search", "web_fetch", "civitai_search", "find_model"] {
            assert!(specs.iter().any(|s| s.name == expected), "{expected} 应当下发：{:?}", specs.iter().map(|s| &s.name).collect::<Vec<_>>());
        }
        // 配方三件（P4 第 67 步）已经实现 → 与桌面那两件一样，必须同时出现在提示词与请求里
        // （在此之前那两条 `absent` 断言是对的：没实现的一律不许下发）
        for expected in ["comfy_learn_workflow", "comfy_list_recipes", "comfy_bind_recipe"] {
            assert!(specs.iter().any(|s| s.name == expected), "{expected} 应当下发");
        }
    }

    /// 联网额度是一条**独立的线**（Kotlin `ToolContext.webInWindow` +
    /// `ToolPolicyConfig.DEFAULT_MAX_WEB_QUERIES_PER_RUN`）：只认联网类工具、
    /// 只在**真跑成功**之后计数、用户答完提问时归零。
    ///
    /// 为什么单列一条用例：这份预算的唯一执行点在执行器里按**工具类别**判，
    /// 而"哪些工具算联网"那份映射在 `ai_settings` —— 两处一漂移，
    /// 系统提示里那句"一次回复最多联网 8 次"就变成空话，**且不会有任何用例变红**。
    #[test]
    fn 联网额度只认联网类工具且只在成功时计数() {
        let mut session = ToolSession::default();
        assert!(session.has_web_queries_left());

        // 非联网工具不占联网额度（否则一次"搜自己的库"就把联网预算吃掉了）
        session.spend_call("search_prompts");
        session.spend_call("comfy_get_status");
        assert_eq!(session.web_queries_in_window, 0);
        assert_eq!(session.calls_in_window, 2, "总调用额度照旧要记");

        session.spend_call("danbooru_lookup");
        assert_eq!(session.web_queries_in_window, 1);
        // `find_model` 也是 WEB 档（P4 第 64 步）—— 它同样吃这一份额度，
        // 所以"这条工具不吃预算"这种漂移会**静默**发生（`ai_settings::category_of`）
        session.spend_call("find_model");
        assert_eq!(session.web_queries_in_window, 2);

        // 吃到上限 → 预检就该拦（执行器回 QUERY_BUDGET_EXCEEDED）
        for _ in 2..crate::policy::MAX_WEB_QUERIES_PER_RUN {
            session.spend_call("danbooru_lookup");
        }
        assert_eq!(session.web_queries_in_window, crate::policy::MAX_WEB_QUERIES_PER_RUN);
        assert!(!session.has_web_queries_left());

        // 用户真答了提问 → 联网额度也归零（与总调用额度同一档重置）
        session.reset_budget();
        assert!(session.has_web_queries_left());
        assert_eq!(session.web_queries_in_window, 0);
    }

    /// **ComfyUI 查询额度**是另一条独立的线（AIH-036「不许把 ComfyUI 当轮询器」，
    /// Kotlin `ToolContext.comfyInWindow` + `DEFAULT_MAX_COMFY_QUERIES_PER_RUN = 9`）。
    ///
    /// 为什么必须有这条用例：系统提示里写着"一次回复最多查 ComfyUI N 次"，
    /// 而在 2026-09-25 那次审查里它**是空的**（Rust 执行器只拦了总调用数与联网数）——
    /// "提示词里写了、代码里没有"这类缺口只有用例钉得住。
    #[test]
    fn comfy_查询额度只认_comfy_类工具且只在成功时计数() {
        let mut session = ToolSession::default();
        assert!(session.has_comfy_queries_left());

        // 其它类别不占这份额度（否则"读一个文件"就把 ComfyUI 预算吃掉了）
        session.spend_call("read_file");
        session.spend_call("search_prompts");
        session.spend_call("danbooru_lookup");
        session.spend_call("load_skill");
        assert_eq!(session.comfy_queries_in_window, 0);
        assert_eq!(session.calls_in_window, 4, "总调用额度照旧要记");

        // comfy_* 三件查询都算（它们是"顺手再确认一下"的重灾区）
        session.spend_call("comfy_get_status");
        session.spend_call("comfy_list_workflows");
        session.spend_call("comfy_get_run");
        assert_eq!(session.comfy_queries_in_window, 3);
        assert_eq!(
            session.web_queries_in_window, 1,
            "danbooru_lookup 走的是**联网**那份额度，两份额度互不串味"
        );

        for _ in 3..crate::policy::MAX_COMFY_QUERIES_PER_RUN {
            session.spend_call("comfy_get_status");
        }
        assert_eq!(session.comfy_queries_in_window, crate::policy::MAX_COMFY_QUERIES_PER_RUN);
        assert!(!session.has_comfy_queries_left(), "到上限后预检必须拦得住");
        assert!(session.has_web_queries_left(), "拦住 ComfyUI 那档不该连累联网额度");

        // ★ **提交不算查询**（2026-09-27 活实例实测出来的那条）：
        //   额度用光之后 `comfy_submit` 必须**照样能走** —— 否则"AI 自己搭一份工作流"这条
        //   正路会在最后一步被挡住（当时它侦察花掉 9 次，工作流写好了却提交不出去）。
        assert!(!crate::ai_settings::is_comfy_query("comfy_submit"));
        assert_eq!(crate::ai_settings::category_of("comfy_submit"), "comfy", "界面分组照旧归 comfy");
        let before = session.comfy_queries_in_window;
        let calls_before = session.calls_in_window;
        session.spend_call("comfy_submit");
        assert_eq!(session.comfy_queries_in_window, before, "提交不占这份额度");
        assert_eq!(session.calls_in_window, calls_before + 1, "但总调用额度照旧要记");
        // 同理：`comfy_use_media` / `comfy_make_mask` 是**写**（投放像素/遮罩），也不算"查询"？
        // —— 不，它们**算**（与 Kotlin 的 ToolCategory.COMFY 一致，而且它们会真的碰实例）。
        assert!(crate::ai_settings::is_comfy_query("comfy_use_media"));
        assert!(crate::ai_settings::is_comfy_query("comfy_make_mask"));

        // 用户真答了提问 → 与联网同一档归零
        session.reset_budget();
        assert!(session.has_comfy_queries_left());
        assert_eq!(session.comfy_queries_in_window, 0);
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
        assert!(is_implemented("web_search"));
        assert!(is_implemented("civitai_search"));
        assert!(is_implemented("remember"));
    }

    /// 桌面文件工具五件（P4 第 40 步）：出厂档 / 写标记必须与 Kotlin 逐条一致
    /// （档位错了的后果是"删 skill 不问一声"或者"写文件被静默禁掉"）。
    #[test]
    fn file_tool_defaults_match_the_kotlin_registry() {
        for name in ["list_dir", "read_file", "write_file", "register_skill"] {
            assert!(is_implemented(name), "{name} 应当已实现");
            assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
        }
        assert_eq!(default_access("delete_skill"), "ASK", "删 skill 要用户点头");
        assert!(is_mutating("write_file"));
        assert!(is_mutating("register_skill"));
        assert!(is_mutating("delete_skill"));
        assert!(!is_mutating("read_file"));
        assert!(!is_mutating("list_dir"));
        // comfy 查询 / 同步四件（P4 第 41~43 步）
        for name in ["comfy_get_run", "comfy_get_status", "comfy_list_workflows", "comfy_load_workflow"] {
            assert!(is_implemented(name), "{name} 应当已实现");
            assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
            assert!(!is_mutating(name), "{name} 是只读的");
        }
        // `comfy_sync_history` 会写库 → 出厂 ASK（用户点头才动）
        assert!(is_implemented("comfy_sync_history"));
        assert_eq!(default_access("comfy_sync_history"), "ASK");
        assert!(is_mutating("comfy_sync_history"));
        // `comfy_submit`（P4 第 48 步，R10）：**会烧显卡时间** → 出厂 ASK，一个字都不能松
        assert!(is_implemented("comfy_submit"));
        assert_eq!(default_access("comfy_submit"), "ASK", "提交必须默认要用户批准");
        assert!(is_mutating("comfy_submit"));
        // `comfy_env_report`（本机模型清单对账）：**只读**，出厂 ALLOW ——
        // 它不属于 COMFY 那一类（只扫盘 + 问一次 /object_info），不该吃掉查询预算
        assert!(is_implemented("comfy_env_report"));
        assert_eq!(default_access("comfy_env_report"), "ALLOW");
        assert!(!is_mutating("comfy_env_report"));
        // `comfy_find_workflow`（两段式找工作流）：**只读**，出厂 ALLOW ——
        // 它是系统提示第 16 条① 点名的那件（在此之前那句话是空的）
        assert!(is_implemented("comfy_find_workflow"));
        assert_eq!(default_access("comfy_find_workflow"), "ALLOW");
        assert!(!is_mutating("comfy_find_workflow"));
        // 体检两件（P4 第 62 步）：**只报不改** → 出厂 ALLOW、mutating=false
        // （Kotlin 那两条写的都是 `mutating = false, defaultAccess = ToolAccess.ALLOW`）
        for name in ["comfy_check_inputs", "comfy_fit_workflow"] {
            assert!(is_implemented(name), "{name} 应当已实现");
            assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
            assert!(!is_mutating(name), "{name} 只报不改");
        }
        // 审核 + 摘拼两件（P4 第 63 步）：同样**只报不改**（摘出来的图是**内联**返回的，
        // 真要落盘得由 write_file 或用户在 ComfyUI 里做）→ 出厂 ALLOW、mutating=false
        for name in ["comfy_audit_workflow", "comfy_prune_workflow"] {
            assert!(is_implemented(name), "{name} 应当已实现");
            assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
            assert!(!is_mutating(name), "{name} 只报不改");
        }
        // 联网三件 + 定向查询（P4 第 64 步）：**出厂全部 ASK**（查询词会离开本机，
        // 用户点一下才发），且全部只读（`download_model` 那条写盘路由保持 deny）。
        // 档位改成 ALLOW 的后果是"查询词静默出本机"，界面上不会有任何变化。
        for name in ["web_search", "web_fetch", "civitai_search", "find_model"] {
            assert!(is_implemented(name), "{name} 应当已实现");
            assert_eq!(default_access(name), "ASK", "{name} 的出厂档必须是 ASK");
            assert!(!is_mutating(name), "{name} 是只读的（绝不代下）");
        }
        // 配方三件（P4 第 67 步）：学是**会写本机配方库**的（所以 mutating=true），
        // 但出厂档与 Kotlin 一致是 ALLOW（知识积累不该每一步都等批准）；
        // 列与绑只读（绑**只报不改**：它不提交、不写文件）。
        for name in ["comfy_learn_workflow", "comfy_list_recipes", "comfy_bind_recipe"] {
            assert!(is_implemented(name), "{name} 已经搬过来了");
            assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
        }
        assert!(is_mutating("comfy_learn_workflow"), "学会往本机配方库写一行");
        assert!(!is_mutating("comfy_list_recipes"));
        assert!(!is_mutating("comfy_bind_recipe"), "绑只报不改");
    }

    /// 设置页那份清单（`implemented`）与下发那份（`catalog`）的差别只有 `deny` 一条。
    #[test]
    fn deny_hides_a_tool_from_the_model_but_not_from_the_settings_page() {
        let mut config = crate::policy::ToolPolicyConfig::default();
        config.overrides.insert("write_file".into(), "deny".into());
        let policy = crate::policy::ToolPolicy::new(std::path::PathBuf::from(r"D:\proj"), config);

        let sent: Vec<&str> = catalog(&policy).iter().map(|t| t.name).collect();
        assert!(!sent.contains(&"write_file"), "被禁用的工具不该下发给模型：{sent:?}");
        let listed: Vec<&str> = implemented().iter().map(|t| t.name).collect();
        assert!(
            listed.contains(&"write_file"),
            "设置页必须能看见被禁用的工具，否则再也开不回来：{listed:?}"
        );
        // 两份都只含已实现的
        assert_eq!(sent.len() + 1, listed.len());
        assert!(listed.contains(&"read_file") && sent.contains(&"read_file"));
    }
}
