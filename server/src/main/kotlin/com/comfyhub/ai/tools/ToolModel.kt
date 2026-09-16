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
    val memory: MemoryStore? = null,
    /** 本 Run 已加载过的 Skill：AIH-040 要求同一份正文不重复塞进上下文 */
    val loadedSkills: MutableSet<String> = mutableSetOf(),
    /** 本 Run 的工具调用计数（AIH-036：不允许无休止轮询） */
    var calls: Int = 0,
    /** 本 Run 里查 ComfyUI 的次数（AIH-036：一次回复最多主动查 3 次） */
    var comfyQueries: Int = 0,
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
