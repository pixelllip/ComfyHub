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

/**
 * 🔒 工具分端（总纲 §3.5 / 红线 R11/R12）。**冻结契约，改它要同时改两端。**
 *
 * ⚠ 这**不是安全边界**（安全边界仍然是 [ToolPolicy]），它回答的是另一个问题：
 * **"别让模型看见一个它根本调不到的工具"** —— 手机上摆着 `comfy_submit`，
 * 模型就会以为它能提交，然后如实汇报"我提交了"，而实际上桌面根本不在线。
 *
 * 默认值刻意是 [DESKTOP]（**不是 BOTH**）：新增一个工具时忘了写 scope，
 * 后果是"手机上少一个工具"（看得见、能发现），而不是"手机上多一个调不到的工具"
 * （看不见、只表现为模型一本正经地胡说）。同理，MCP 拉来的 39 个工具也自动落在 DESKTOP。
 */
enum class ToolScope(val wire: String) {
    /**
     * "两端合起来"的全集 —— **只在桌面自己跑 Run 时用**（`specs(policy)` 的默认值）。
     *
     * 它的存在是为了让 `ToolScope` 这一步**纯加字段、零行为变化**：桌面今天的 Run
     * 拿到的清单与加这个枚举之前逐字相同。手机侧传 [MOBILE]。
     */
    ALL("ALL"),

    /** 只有桌面能履行：comfy_*、MCP 的 39 个、本机文件读写、skill 注册/删除 */
    DESKTOP("DESKTOP"),

    /** 手机 harness 自己能做（移植过去的那一套），桌面也能做 */
    BOTH("BOTH"),

    /** 只有手机有（例如手机独有的 `comfy_queue_submit`，R11） */
    MOBILE("MOBILE"),
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

    /**
     * **本机资料库**查询（O7/O8，2026-09-25）：搜自己的提示词库、读写角色卡。
     *
     * 单独一类而不是塞进 [MEMORY]：记忆是"一行一条文本"、有 100 条上限、
     * 超限直接报错，而这里的东西是**结构化**的（角色卡有 tag 栈 / LoRA / 权重 / 来源，
     * 提示词库有正负提示词与参数）。分开的另一个理由：用户可能想整体关掉
     * "别再翻我的库"，而保留长期记忆。
     */
    LIBRARY("library"),
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
     * **可重置的调用预算**（用户建议 2026-09-22：每完成一次选择就重置工具预算）。
     *
     * 为什么要有它：定稿前的每一步都要问用户（用哪张参考图 / 装不装这个模型 / 摘不摘这条分支），
     * 用户答一次就该有一次全新的额度 —— 否则他会看到"我刚回答完你的问题，你就说预算用尽、
     * 让我再发一条消息"（2026-09-21 的对话里连着撞了三次，见 `bug-and-suggestion-9.22.md` 建议 ③）。
     *
     * 与 [calls] 的关系：[calls] 是**整个 Run 的绝对总数**（只增不减，硬天花板见
     * `ToolRegistry.ABSOLUTE_CALL_CAP`）；这里是**当前这一窗**的用量，
     * 用户回答提问时归零（[resetBudget]）。`ask_user_question` **自己不占额度**
     * —— 它只把人拉回环路，不是往世界上做事；联网 / ComfyUI 那两个专项预算同理放在窗里。
     */
    var callsInWindow: Int = 0,
    /** 当前这一窗里查 ComfyUI 的次数（用户回答提问后同样归零）。 */
    var comfyInWindow: Int = 0,
    /** 当前这一窗里联网搜索的次数（同上）。 */
    var webInWindow: Int = 0,
    /**
     * 当前这一窗里**已经开过几张提问卡**（2026-09-22 用户拍的"阈值制"：
     * 不问是常态，所以硬预算很紧 —— 每窗最多一张卡，卡里最多 5 问）。
     *
     * 为什么不跟 [callsInWindow] 一起算：一张卡最多能问 5 件事，如果按"一次调用"计数，
     * "少问、问全"就变成了"多调几次、每次一问"。这个计数盯的是**卡**，不是调用。
     */
    var askCardsInWindow: Int = 0,
    /**
     * 本 Run 里**因为用户回答了提问**而重置过几次预算。
     *
     * 有上限（[MAX_BUDGET_RESETS]）：用户一直答就该一直给，但"一直答"不该变成一条
     * 无限预算的旁路 —— 到顶之后不再重置，如实告诉用户"再发一条消息接着做"。
     */
    var budgetResets: Int = 0,
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
    /**
     * **本 Run 里真的跟 danbooru 核对过的 tag**（O8，2026-09-25）。
     *
     * `danbooru_lookup` 查到一个角色 tag 就往这里记一笔；`save_character_card` 只在这个
     * 集合里有对应 tag 时才写 `tags_verified_at`。
     *
     * 为什么要有它：`tags_verified_at` 的语义是"这张卡的 tag 跟 danbooru 核对过" ——
     * 如果让模型自己填一个时间戳，那它就成了模型的一句**自称**（与"我查过了"没有区别）。
     * 这里把它变成**这一轮的事实**：没查过就写不出核对时间，卡片上也就看得见差别。
     */
    val danbooruVerified: MutableSet<String> = mutableSetOf(),
) {
    /** 本 Run 还能不能再动一次工具（窗内额度与绝对天花板都没到）。 */
    val hasCallsLeft: Boolean get() = callsInWindow < windowLimit && calls < ToolRegistry.ABSOLUTE_CALL_CAP

    /**
     * 当前这一窗的调用上限。测试里构造的 [ToolPolicy] 没有真配置时回落到出厂值。
     */
    private val windowLimit: Int
        get() = policy.config.maxCallsPerRun.coerceAtLeast(1)

    /**
     * 记一次**真的执行了**的工具调用。
     *
     * 只在"过完预算检查 + 权限档检查 + 审批"之后调 —— 被拒、越界、工具自己报错
     * 都不该消耗用户的额度（那些调用连世界都没碰到）。
     */
    fun spendCall(category: ToolCategory?) {
        calls++
        callsInWindow++
        when (category) {
            ToolCategory.COMFY -> {
                comfyQueries++
                comfyInWindow++
            }
            ToolCategory.WEB -> {
                webQueries++
                webInWindow++
            }
            else -> Unit
        }
    }

    /**
     * 用户刚刚**回答了提问**：把这一窗的预算归零，让他接着把这件事做完
     * （用户建议 ③ 的原话："每次完成选择后，重置工具预算"）。
     *
     * 返回 false = 重置次数已经用尽（[MAX_BUDGET_RESETS]），调用方要如实说明，
     * 而不是假装额度又满了。提问工具本身不占额度，所以这里不会"重置完就欠一笔"。
     */
    fun resetBudget(): Boolean {
        if (budgetResets >= MAX_BUDGET_RESETS) return false
        budgetResets++
        callsInWindow = 0
        comfyInWindow = 0
        webInWindow = 0
        // 用户答完提问 = 他要接着往下做了，所以"这一窗开过几张卡"也归零
        // （否则一次澄清之后模型想再确认一件事都开不了卡）
        askCardsInWindow = 0
        return true
    }

    companion object {
        /**
         * 一次 Run 里最多因"用户回答了提问"重置几次预算。
         *
         * 4 次的依据：问 4 次已经是一轮里非常啰嗦的对话了（正常一次提问就够），
         * 再往上就该让用户自己说话、而不是无限续额度。
         */
        const val MAX_BUDGET_RESETS = 4

        /**
         * 一窗（一次回复）最多开几张提问卡 —— 用户 2026-09-22 拍的"阈值制"：
         * **不问是常态**，实在要问的关键问题才许问，所以这个数字很小。
         */
        const val MAX_ASK_CARDS_PER_WINDOW = 1

        /** 一张卡最多几个问题（用户拍板：5）。 */
        const val MAX_QUESTIONS_PER_CARD = 5
    }
}

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
    /**
     * 这个工具在哪一端可达（🔒 [ToolScope]，总纲 §3.5）。
     * 默认 [ToolScope.DESKTOP] —— 见枚举上的注释：宁可手机上少一个，也不能多一个调不到的。
     */
    val scope: ToolScope = ToolScope.DESKTOP,
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
