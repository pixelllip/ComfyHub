package com.comfyhub.ai.tools

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject

/**
 * 工具层的公共模型（M4）。
 *
 * 设计原则（与实施方案 §10 一致）：
 *  - 工具**只能**通过 [AgentTool.handler] 访问世界，参数一律先过 [ToolPolicy]；
 *  - 只读工具可以直接执行，写工具按策略 `ask`/`deny`（AIH-035 / DEC-005）；
 *  - 工具输出对模型是**不可信数据**：截断、脱敏、不解释；
 *  - 首期**没有** shell / 进程类工具（AIH-045），文件工具也只在允许的目录里活动。
 */

/** 工具的可信度/权限档（用户可在设置里逐项覆盖）。 */
enum class ToolAccess(val wire: String) {
    ALLOW("allow"),
    ASK("ask"),
    DENY("deny");

    companion object {
        fun parse(wire: String?): ToolAccess? = entries.firstOrNull { it.wire == wire }
    }
}

/** 工具分类，仅用于界面分组与文档。 */
enum class ToolCategory(val wire: String) {
    SKILL("skill"),
    FILES("files"),
    COMFY("comfy"),
    MEMORY("memory"),

    /**
     * 联网搜索（2026-09-20 新增）。
     *
     * 单独一类而不是塞进 [FILES]：它是**唯一会把数据送出本机**的工具类别，
     * 界面要能一眼分组、用户要能整类关掉（`deny`），预算也是按类单独算的。
     */
    WEB("web"),

    /**
     * 提问（用户建议 ⑥，2026-09-20）。
     *
     * 单独一类是因为它**碰的不是世界，是人**：它不读写文件、不查 ComfyUI、不出网，
     * 只是把一个问题递到用户面前并把回答带回来。界面上也应当与"工具卡"分开画
     * （见 `_AskCard`），否则用户会把"等你回答"误当成"等你批准"。
     */
    ASK("ask"),
    ;

    companion object {
        fun parse(wire: String?): ToolCategory? = entries.firstOrNull { it.wire == wire }
    }
}

/**
 * 一次工具调用的上下文。
 *
 * [runId] 用于审批与审计；[policy] 是**本次 Run 生效**的策略快照（用户中途改设置不影响在飞的 Run）。
 */
class ToolContext(
    val runId: String,
    val policy: ToolPolicy,
    val skills: SkillStore,
    /** 长期记忆（M6）；为 null 表示本次部署没启用 */
    val memory: MemoryStore? = null,    /** 本 Run 已加载过的 Skill：AIH-040 要求同一份正文不重复塞进上下文 */
    val loadedSkills: MutableSet<String> = mutableSetOf(),
    /** 本 Run 的工具调用计数（AIH-036：不允许无休止轮询） */
    var calls: Int = 0,
    /** 本 Run 里查 ComfyUI 的次数（预算见 `ToolPolicyConfig.DEFAULT_MAX_COMFY_QUERIES_PER_RUN`） */
    var comfyQueries: Int = 0,
    /**
     * 本 Run 里**联网搜索**的次数（[ToolCategory.WEB]，预算见
     * `ToolPolicyConfig.maxWebQueriesPerRun`）。
     *
     * 搜一次就是一次外部请求 + 一份要烧 token 的正文，模型的"再搜一下确认"
     * 很容易刹不住车，所以和 ComfyUI 一样有独立预算，不共用 [calls]。
     */
    var webQueries: Int = 0,
    /**
     * 本次请求里是否带了图片附件（M3）。
     *
     * `comfy_check_inputs` 用它区分"这个图片输入必须问用户要"和"用户刚发了图、
     * 投放一下就能解决" —— 两者混在一起，模型就会去问用户要一张它已经收到的图。
     */
    val hasImageAttachment: Boolean = false,
    /**
     * 本 Run 里**真的新增**了几条长期记忆（用户要求：一次最多
     * [MemoryStore.MAX_ENTRIES_PER_RUN] 条）。
     *
     * 记的是"新增成功"的次数：模型把同一件事说两遍时 `append` 会判重直接返回，
     * 那种不算，不然模型会因为自己啰嗦而被拒。
     */
    var memoryWrites: Int = 0,
    /**
     * 本 Run 所属的会话 id（用户建议 ⑥ 的提问要用它把提问挂到正确的那条对话上）。
     */
    val conversationId: String = "",
    /**
     * 往这次 Run 的事件流里发一条事件（由 HarnessRunner 注入，签名与它内部的 emit 一致）。
     *
     * `ask_user_question` 靠它把提问送到界面；测试里可以传一个只记下来不真发的实现。
     */
    val emit: ((String, JsonObject) -> Unit)? = null,
)

/** 工具执行结果。 */
data class ToolOutput(
    val content: String,
    val meta: JsonObject? = null,
)

/** 工具主动报错（带稳定 code，会被写进 `ai_tool_calls.error` 与 `tool.failed` 事件）。 */
class ToolFailure(val code: String, message: String) : RuntimeException(message)

/** 工具定义 + 执行体。 */
class AgentTool(
    val name: String,
    val description: String,
    val parameters: JsonObject,
    val category: ToolCategory,
    val mutating: Boolean,
    /** 出厂默认权限档；用户设置里的覆盖优先（见 [ToolPolicy.accessFor]） */
    val defaultAccess: ToolAccess,
    val handler: suspend (JsonObject, ToolContext) -> ToolOutput,
)

/** 工具执行完的完整记录（同时用于 SSE 事件与 `ai_tool_calls` 落库）。 */
data class ToolCallRecord(
    val id: String,
    val runId: String,
    val callId: String,
    val name: String,
    val argumentsJson: String,
    /** not_required / pending / approved / denied */
    val approval: String,
    /** ok / failed / denied */
    val status: String,
    val resultJson: JsonObject? = null,
    val content: String,
    val preview: String,
    val error: String? = null,
    val errorCode: String? = null,
    val elapsedMs: Long = 0,
) {
    /** 是不是成功执行（`denied` / `failed` 都不算）。 */
    val ok: Boolean get() = status == "ok"
}

/** 工具清单 DTO（界面用：设置页与右侧栏要能显示"有哪些工具、什么权限"）。 */
@Serializable
data class ToolInfoDto(
    val name: String,
    val description: String,
    val category: String,
    val mutating: Boolean,
    /** 生效权限：allow / ask / deny */
    val access: String,
    /** 是否被用户设置覆盖过 */
    val overridden: Boolean = false,
)

/**
 * `comfy_find_workflow` 的搜索结果（用户建议 ①）。
 *
 * [json] 是回给模型与界面用的完整结构；[count] 单独留一份，方便工具在"一条都没找到"时
 * 用 `NOT_FOUND` 失败（模型看到失败码比看到 `count: 0` 更容易纠正自己的做法）。
 */
data class WorkflowSearch(
    val count: Int,
    val json: JsonObject,
)

/**
 * `comfy_submit` 的结果（用户建议 ①）。
 *
 * [mediaIds] 让界面能在助手回复末尾直接贴上**画廊入口卡**（用户建议 ⑤）：
 * 模型拿到的 `json` 里也有它们，但界面不应该去解析工具结果 JSON。
 */
data class ComfySubmitOutcome(
    val promptId: String,
    val status: String,
    val title: String?,
    val mediaIds: List<Long> = emptyList(),
    val capturedPromptId: Long? = null,
    val json: JsonObject,
)
