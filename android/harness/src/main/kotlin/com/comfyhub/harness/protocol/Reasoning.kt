package com.comfyhub.harness.protocol

/**
 * 思考强度（reasoning effort）的协议适配 —— 移植自桌面
 * `com.comfyhub.ai.protocol.Reasoning.kt`（AIH-056），逐行对照。
 *
 * 同一个"高/中/低"在不同协议、不同网关上落到的字段完全不一样，所以这里把"用户选的等级"
 * 与"线上真正发的值"分开：
 *
 * | 协议 / thinkingFormat | 请求体字段 |
 * | --- | --- |
 * | anthropic-messages | `thinking{type,budget_tokens}`，`max_tokens` 必须大于预算 |
 * | openai + deepseek | `thinking{type:enabled/disabled}` + `reasoning_effort` |
 * | openai + qwen | `enable_thinking` + `reasoning_effort` |
 * | openai + openrouter | `reasoning{effort}`（"none" 表示关闭） |
 * | openai + zai | `thinking{type,clear_thinking}` + `reasoning_effort` |
 * | openai（默认 openai 方言） | `reasoning_effort`；不指定就是不带这个字段 |
 *
 * **模型没声明推理能力时一律不发这些字段** —— 往不支持推理参数的模型上塞
 * `reasoning_effort` 会被网关 400，宁可不发。
 *
 * ## [ReasoningEffort.OFF] 是"不指定"，不是"一定不思考"
 *
 * 界面上这一档叫**「默认」**。真机核对过的网关（本机在用的 `command-code-goat`）
 * `reasoning_effort` 只认 `low|medium|high|xhigh|max`，而 `thinking{type:disabled}` /
 * `enable_thinking:false` / `reasoning{enabled:false}` 一律被忽略 —— 照样思考。所以：
 *  · 对 `deepseek` / `zai` / `openrouter` 这些**默认开思考**的方言，我们仍然显式发"禁用"
 *    （那是我们能做的最好），但**不许对外承诺"已经关掉了"**；
 *  · 对 `openai` 方言，一个字段都不发，思考与否完全由网关决定。
 */

/** 一个可选的思考等级。`off` 表示"**不指定**思考强度"（界面上叫「默认」）。 */
enum class ReasoningEffort(val wire: String) {
    OFF("off"),
    MINIMAL("minimal"),
    LOW("low"),
    MEDIUM("medium"),
    HIGH("high"),

    /**
     * 比 high 更高、但还没到厂商上限的一档。
     *
     * 它不是我们发明的：等级表就是 `off → minimal → low → medium → high → xhigh → max`，
     * 而且不少前沿模型**只声明到 xhigh**——没有这一档就只能把它们压回 high，
     * 用户以为选了"极高"其实发出去的是 high，比报错更糟。
     */
    XHIGH("xhigh"),
    MAX("max");

    companion object {
        fun parse(raw: String?): ReasoningEffort? {
            val v = raw?.trim()?.lowercase() ?: return null
            if (v.isEmpty()) return null
            return entries.firstOrNull { it.wire == v }
        }
    }
}

/** 线上怎么表达"思考等级"（DIALECT）。模型目录可以用 `thinkingFormat` 覆盖默认值。 */
enum class ThinkingFormat(val wire: String) {
    /** OpenAI 官方风格：直接 `reasoning_effort`；关闭 = 不带该字段。 */
    OPENAI("openai"),
    /** DeepSeek 风格：`thinking{type}` + `reasoning_effort`。 */
    DEEPSEEK("deepseek"),
    /** 通义/Qwen 系：`enable_thinking` + `reasoning_effort`。 */
    QWEN("qwen"),
    /** OpenRouter 归一化：`reasoning{effort}`，"none" 表示关闭。 */
    OPENROUTER("openrouter"),
    /** 智谱 Z.ai：`thinking{type:enabled,clear_thinking:false}` + `reasoning_effort`。 */
    ZAI("zai");

    companion object {
        fun parse(raw: String?): ThinkingFormat? {
            val v = raw?.trim()?.lowercase() ?: return null
            if (v.isEmpty()) return null
            return entries.firstOrNull { it.wire == v }
        }
    }
}

/** 每个等级默认对应的"过线拼写"。 */
object ThinkingLevels {
    val DEFAULT: Map<ReasoningEffort, String> = mapOf(
        ReasoningEffort.OFF to "none",
        ReasoningEffort.MINIMAL to "minimal",
        ReasoningEffort.LOW to "low",
        ReasoningEffort.MEDIUM to "medium",
        ReasoningEffort.HIGH to "high",
        ReasoningEffort.XHIGH to "xhigh",
        // 只有 `max` 是"收敛"的：Responses 早期没有 max 档，客户端会按协议再收一次
        ReasoningEffort.MAX to "high",
    )

    /** 每个等级的默认思考预算（仅 Anthropic 需要具体 token 数）。 */
    val ANTHROPIC_BUDGET: Map<ReasoningEffort, Int> = mapOf(
        ReasoningEffort.MINIMAL to 1024,
        ReasoningEffort.LOW to 2048,
        ReasoningEffort.MEDIUM to 8192,
        ReasoningEffort.HIGH to 16384,
        ReasoningEffort.XHIGH to 24576,
        ReasoningEffort.MAX to 32768,
    )

    const val ANTHROPIC_MIN_BUDGET = 1024
}

/**
 * 一次请求的思考设置（已经过"模型是否声明推理能力"过滤）。
 *
 * @param effort 用户选的等级，`null` 表示这次**不指定**
 * @param wireValue 实际要发的值（默认取 [ThinkingLevels.DEFAULT]，模型可用 `reasoningEfforts` 改名）
 * @param format 网关方言
 * @param budgetTokens Anthropic 的思考预算；数字直接写在 `reasoningEfforts` 里时也可以带值
 */
data class ReasoningRequest(
    val effort: ReasoningEffort?,
    val wireValue: String?,
    val format: ThinkingFormat,
    val budgetTokens: Int? = null,
) {
    val enabled: Boolean get() = effort != null

    companion object {
        /** 完全不发任何思考字段（也**没有**任何方言信息）。 */
        val NONE = ReasoningRequest(null, null, ThinkingFormat.OPENAI)

        /**
         * @param requested 用户选的等级；`off` 与 `null` 都表示"不指定"（界面上叫「默认」）
         * @param declaredReasoning 模型目录是否声明了推理能力 —— 没声明就直接 [NONE]
         * @param levelMap 该模型自定义的"等级 → 过线拼写"，值可以是字符串（改名）或数字（预算）
         * @param format 网关方言。**即使不指定也要带上**：DeepSeek / Z.AI / OpenRouter
         *   这类默认开思考的网关，我们会试着显式发"禁用"（网关未必认，见文件头）。
         */
        fun of(
            requested: ReasoningEffort?,
            declaredReasoning: Boolean,
            levelMap: Map<String, LevelSpec> = emptyMap(),
            format: ThinkingFormat = ThinkingFormat.OPENAI,
        ): ReasoningRequest {
            if (!declaredReasoning) return NONE
            if (requested == null || requested == ReasoningEffort.OFF) {
                return ReasoningRequest(null, null, format)
            }
            val spec = levelMap[requested.wire]
            val wireValue = when (spec) {
                is LevelSpec.Name -> spec.value
                else -> ThinkingLevels.DEFAULT[requested]
            }
            val budget = when (spec) {
                is LevelSpec.Budget -> spec.value
                else -> ThinkingLevels.ANTHROPIC_BUDGET[requested]
            }
            return ReasoningRequest(requested, wireValue, format, budget)
        }
    }
}

/** 模型对某个等级的自定义表达：要么改名，要么直接给 token 预算。 */
sealed interface LevelSpec {
    data class Name(val value: String) : LevelSpec
    data class Budget(val value: Int) : LevelSpec

    companion object {
        fun of(raw: String?): LevelSpec? {
            val v = raw?.trim() ?: return null
            if (v.isEmpty()) return null
            val n = v.toIntOrNull()
            if (n != null) return if (n <= 0) null else Budget(n)
            return Name(v)
        }
    }
}
