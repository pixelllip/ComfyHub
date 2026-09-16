package com.comfyhub.ai

/**
 * 能力来源（AIH-011）。**每一项都会显示给用户**，绝不把来源藏在后台。
 */
enum class CapabilitySource(val wire: String, val label: String) {
    /** 接口自己声明的（最可信） */
    DISCOVERED("discovered", "接口声明"),
    /** 内置目录：按厂商公开文档整理的离线表，可能过期 */
    BUILTIN("builtin", "内置目录"),
    /** 用户手工填写 */
    MANUAL("manual", "手工声明"),
    /** 用真实请求探测过 */
    TESTED("tested", "已实测"),
}

/**
 * 内置模型能力目录。
 *
 * 需求里明确"不能根据 model id 猜能力"，这条依然成立：这里**不是猜**，而是一张
 * 按厂商公开文档整理的、带版本的**离线表**，并且：
 *
 *  - 只在接口没有给出能力信息时才使用（接口声明永远优先）；
 *  - 命中的模型在界面上标成「内置目录」，来源可见、可手工改；
 *  - **表里没有的模型一律按"仅文本"处理**，绝不用"名字看着像"去推断图片能力；
 *  - 表可能过期（新模型层出不穷），过期只影响"默认勾选"，用户改一下就覆盖。
 *
 * 登记三样东西：输入模态、工具、推理（含可选的**思考档位**）。
 * 视频 / 音频不做预填：适配器还不支持这些传输方式，勾了也只会被正确阻断。
 */
object ModelCapabilityCatalog {
    const val VERSION = "2026-09b"

    data class Capability(
        val modalities: List<String>,
        val tools: Boolean,
        val reasoning: Boolean,
        /** 命中的规则，便于向用户解释"为什么给它勾了图片" */
        val matchedBy: String,
        /**
         * 预填的思考档位（AIH-056）：等级 → 线上表达，空表示"声明了推理但不预填档位"。
         *
         * 只在 [reasoning] 为 true 时有意义，且必须与之一致 ——
         * 保存时 `validateThinkingEfforts` 会拒绝"声明了档位却没勾推理"。
         */
        val thinkingEfforts: Map<String, String> = emptyMap(),
        /** 预填的思考方言（网关字段差异）；null = 按协议默认。 */
        val thinkingFormat: String? = null,
    )

    private val TEXT_ONLY = listOf("text")
    private val TEXT_IMAGE = listOf("text", "image")

    // 常见的思考档位组合。等级与 pi-ai / DSH 的等级表一致
    // （off → minimal → low → medium → high → xhigh → max），值是**线上表达**。
    //
    // 这些组合**不是拍脑袋写的**：与 `%USERPROFILE%\.dsh\settings.yaml` 里各模型的
    // `reasoningEfforts` 一致（那是本机在用的权威声明，见下面的规则表）。
    private val EFFORT_MIN_LOW_MED_HIGH =
        mapOf("off" to "none", "minimal" to "minimal", "low" to "low", "medium" to "medium", "high" to "high")
    private val EFFORT_LOW_MED_HIGH =
        mapOf("off" to "none", "low" to "low", "medium" to "medium", "high" to "high")
    private val EFFORT_LOW_HIGH = mapOf("off" to "none", "low" to "low", "high" to "high")
    private val EFFORT_HIGH_ONLY = mapOf("off" to "none", "high" to "high")
    private val EFFORT_HIGH_MAX = mapOf("off" to "none", "high" to "high", "max" to "max")
    private val EFFORT_LOW_HIGH_MAX =
        mapOf("off" to "none", "low" to "low", "high" to "high", "max" to "max")
    private val EFFORT_LOW_MED_HIGH_MAX =
        mapOf("off" to "none", "low" to "low", "medium" to "medium", "high" to "high", "max" to "max")
    /** 只有"极高/最大"两档的（Fugu Ultra 这类）。 */
    private val EFFORT_HIGH_XHIGH = mapOf("off" to "none", "high" to "high", "xhigh" to "xhigh")
    /** 带 xhigh 但没有 max：GPT-5.4/5.5/5.3-codex、Grok 4.6、Muse Spark 这些。 */
    private val EFFORT_LOW_MED_HIGH_XHIGH =
        mapOf("off" to "none", "low" to "low", "medium" to "medium", "high" to "high", "xhigh" to "xhigh")
    /** 满档：low → xhigh 全给，再加口头上的"最大"。 */
    private val EFFORT_LOW_MED_HIGH_XHIGH_MAX = EFFORT_LOW_MED_HIGH_XHIGH + ("max" to "max")
    private val EFFORT_LOW_MED_XHIGH =
        mapOf("off" to "none", "low" to "low", "medium" to "medium", "xhigh" to "xhigh")

    /** 前缀/包含匹配的规则表；顺序从上到下，先命中先用。 */
    private val rules: List<Triple<String, Capability, Boolean>> = buildList {
        fun rule(
            pattern: String,
            modalities: List<String>,
            tools: Boolean,
            reasoning: Boolean = false,
            efforts: Map<String, String> = emptyMap(),
            format: String? = null,
        ) {
            add(
                Triple(
                    pattern,
                    Capability(modalities, tools, reasoning, matchedBy = pattern,
                        thinkingEfforts = efforts, thinkingFormat = format),
                    true,
                )
            )
        }

        // ------------------------------------------------------------------
        //  与 %USERPROFILE%\.dsh\settings.yaml 对齐的"常用模型"规则
        //
        //  用户明确要求：导入模型时别再让他一个个手填模态与思考档位。
        //  下面每条都能在 settings.yaml 里找到对应条目（`input` / `reasoningEfforts`）。
        //  注意 lookup() 是**包含匹配**且大小写不敏感，所以 id 里带 `org/` 前缀也能命中。
        // ------------------------------------------------------------------

        // --- 明确带图片输入、且声明了思考档位的系列 ---
        rule("claude-sonnet-5", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH_MAX)
        rule("claude-sonnet-4-6", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH_MAX)
        rule("claude-fable", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH_MAX)
        rule("claude-opus-5", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH_MAX)
        rule("claude-opus-4-8", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH_MAX)
        rule("claude-opus-4-7", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH_MAX)
        // Haiku 4.5 在 settings.yaml 里没有 reasoningEfforts（不支持思考），只给图片
        rule("claude-haiku-4", TEXT_IMAGE, tools = true)

        rule("gpt-5.6", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH_MAX)
        rule("gpt-5.5", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH)
        rule("gpt-5.4-mini", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
        rule("gpt-5.4", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH)
        rule("gpt-5.3", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH)

        rule("deepseek-v4.1", TEXT_IMAGE, tools = true, reasoning = true,
            efforts = EFFORT_LOW_HIGH_MAX, format = "deepseek")
        // 只有 vision-exp 带图片：放在上面那条之前，先命中
        rule("flash-vision", TEXT_IMAGE, tools = true, reasoning = true,
            efforts = EFFORT_HIGH_MAX, format = "deepseek")
        rule("deepseek-v4", TEXT_ONLY, tools = true, reasoning = true,
            efforts = EFFORT_HIGH_MAX, format = "deepseek")
        rule("deepseek-v3", TEXT_ONLY, tools = true, format = "deepseek")
        rule("deepseek-r1", TEXT_ONLY, tools = true, reasoning = true,
            efforts = EFFORT_HIGH_ONLY, format = "deepseek")

        rule("kimi-k3", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_HIGH_MAX)
        rule("kimi-k2.7", TEXT_IMAGE, tools = true)
        rule("kimi-k2.6", TEXT_IMAGE, tools = true)
        rule("kimi-k2.5", TEXT_IMAGE, tools = true)

        rule("glm-5.3", TEXT_IMAGE, tools = true, reasoning = true,
            efforts = EFFORT_LOW_HIGH_MAX, format = "zai")
        rule("glm-5.2", TEXT_ONLY, tools = true, reasoning = true,
            efforts = EFFORT_HIGH_MAX, format = "zai")
        rule("glm-5", TEXT_ONLY, tools = true, format = "zai")

        rule("minimax-m3", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
        rule("minimax-m2", TEXT_ONLY, tools = true)

        rule("mimo-v2.5", TEXT_IMAGE, tools = true)
        rule("mimo", TEXT_ONLY, tools = true)

        rule("qwen3.8", TEXT_IMAGE, tools = true, reasoning = true,
            efforts = EFFORT_LOW_MED_XHIGH, format = "qwen")
        rule("qwen3.7", TEXT_IMAGE, tools = true, format = "qwen")
        // Qwen3.6 及更早的 Qwen3 系（3.6-Plus / 3.7-Plus 这些都能吃图）
        rule("qwen3", TEXT_IMAGE, tools = true, format = "qwen")

        rule("step-3.7", TEXT_IMAGE, tools = true)
        rule("step", TEXT_ONLY, tools = true)
        rule("hy4", TEXT_ONLY, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
        rule("hy3", TEXT_ONLY, tools = true)

        rule("gemini-3", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
        rule("fugu-ultra", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_HIGH_XHIGH)
        rule("muse-spark-1.2", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH)
        rule("muse-spark-1.3", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH_MAX)
        rule("muse-spark-1.1", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH)
        rule("muse-spark", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH)
        rule("grok-4.6", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_XHIGH)
        rule("grok-4.5", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
        rule("inkling", TEXT_IMAGE, tools = true)
        rule("nemotron", TEXT_ONLY, tools = true)
        rule("longcat", TEXT_ONLY, tools = true)
        rule("ling-3", TEXT_ONLY, tools = true)
        rule("laguna", TEXT_ONLY, tools = true)
        rule("gpt-oss", TEXT_ONLY, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
        // Claude 4.6 及更早的通用规则（顺序在 claude-*-5 之后）
        rule("claude-4-6", TEXT_IMAGE, tools = true)

        // ------------------------------------------------------------------
        //  通用规则（历史条目，保持兼容）
        // ------------------------------------------------------------------

        // --- 明确的多模态系列（图片输入） ---
        rule("gpt-4o", TEXT_IMAGE, tools = true)
        rule("gpt-4.1", TEXT_IMAGE, tools = true)
        rule("gpt-4-turbo", TEXT_IMAGE, tools = true)
        rule("gpt-4-vision", TEXT_IMAGE, tools = true)
        rule("chatgpt-4o", TEXT_IMAGE, tools = true)
        rule("o3", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
        rule("o4-mini", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
        rule("claude-3", TEXT_IMAGE, tools = true)
        rule("claude-4", TEXT_IMAGE, tools = true)
        rule("gemini-1.5", TEXT_IMAGE, tools = true)
        rule("gemini-2", TEXT_IMAGE, tools = true)
        rule("gemini-pro-vision", TEXT_IMAGE, tools = true)
        rule("qwen-vl", TEXT_IMAGE, tools = true)
        rule("qwen2-vl", TEXT_IMAGE, tools = true)
        rule("qwen2.5-vl", TEXT_IMAGE, tools = true)
        rule("qwen3-vl", TEXT_IMAGE, tools = true)
        rule("qwen-omni", TEXT_IMAGE, tools = true)
        rule("llava", TEXT_IMAGE, tools = false)
        rule("bakllava", TEXT_IMAGE, tools = false)
        rule("moondream", TEXT_IMAGE, tools = false)
        rule("minicpm-v", TEXT_IMAGE, tools = false)
        rule("internvl", TEXT_IMAGE, tools = false)
        rule("glm-4v", TEXT_IMAGE, tools = true)
        rule("glm-4.1v", TEXT_IMAGE, tools = true)
        rule("pixtral", TEXT_IMAGE, tools = true)
        rule("phi-3-vision", TEXT_IMAGE, tools = false)
        rule("phi-4-multimodal", TEXT_IMAGE, tools = false)
        rule("deepseek-vl", TEXT_IMAGE, tools = false)
        rule("yi-vision", TEXT_IMAGE, tools = false)
        rule("step-1v", TEXT_IMAGE, tools = false)
        rule("cogvlm", TEXT_IMAGE, tools = false)
        rule("florence", TEXT_IMAGE, tools = false)
        rule("grok-2-vision", TEXT_IMAGE, tools = true)
        rule("grok-4", TEXT_IMAGE, tools = true)
        rule("mistral-medium-3", TEXT_IMAGE, tools = true)

        // --- OpenAI 的 GPT-5 系列：默认就带思考，档位含 minimal ---
        rule("gpt-5", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_MIN_LOW_MED_HIGH)

        // --- 明确只吃文本的（避免被上面更宽的前缀误伤，也为界面提供"确定不支持"的结论） ---
        // DeepSeek 的思考开关是 `thinking{type}`，且**默认开**，所以关闭要显式发 disabled
        rule("deepseek-reasoner", TEXT_ONLY, tools = true, reasoning = true,
            efforts = EFFORT_HIGH_ONLY, format = "deepseek")
        rule("deepseek-chat", TEXT_ONLY, tools = true, format = "deepseek")
        rule("deepseek-coder", TEXT_ONLY, tools = false, format = "deepseek")
        rule("qwq", TEXT_ONLY, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH,
            format = "qwen")
        rule("o1-mini", TEXT_ONLY, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
        rule("embedding", TEXT_ONLY, tools = false)
        rule("whisper", TEXT_ONLY, tools = false)
        rule("tts", TEXT_ONLY, tools = false)
        rule("dall-e", TEXT_ONLY, tools = false)
        rule("moderation", TEXT_ONLY, tools = false)

        // --- 常见文本系列（含工具能力） ---
        rule("gpt-3.5", TEXT_ONLY, tools = true)
        rule("gpt-4", TEXT_ONLY, tools = true)
        rule("o1", TEXT_ONLY, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
        rule("o3-mini", TEXT_ONLY, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
        // GLM / Qwen 的思考是 `enable_thinking` 或 `thinking{type}`，默认可能开
        rule("glm-4", TEXT_ONLY, tools = true, format = "zai")
        rule("glm-3", TEXT_ONLY, tools = true, format = "zai")
        rule("glm", TEXT_ONLY, tools = true, format = "zai")
        rule("qwen", TEXT_ONLY, tools = true, format = "qwen")
        rule("moonshot", TEXT_ONLY, tools = true)
        rule("kimi", TEXT_ONLY, tools = true)
        rule("mistral", TEXT_ONLY, tools = true)
        rule("mixtral", TEXT_ONLY, tools = true)
        rule("llama-3", TEXT_ONLY, tools = true)
        rule("llama-4", TEXT_IMAGE, tools = true)
        rule("command-r", TEXT_ONLY, tools = true)
        rule("ernie", TEXT_ONLY, tools = true)
        rule("hunyuan", TEXT_ONLY, tools = true)
        rule("spark", TEXT_ONLY, tools = true)
        rule("doubao", TEXT_ONLY, tools = true)
        rule("claude", TEXT_ONLY, tools = true)
        rule("gemini", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_MAX)
        rule("grok", TEXT_ONLY, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
    }

    /** 查表；返回 null 表示"未知"，调用方必须按保守默认处理。 */
    fun lookup(modelId: String): Capability? {
        val id = modelId.lowercase()
        // 包含匹配：宽前缀规则排在后面，避免 `gpt-4` 抢走 `gpt-4o`
        for ((pattern, cap, contains) in rules) {
            if (contains && id.contains(pattern)) return cap
        }
        return null
    }

    /**
     * 保守默认：目录里没有、接口也没说 → 只给文本。
     * 图片要用户自己勾（勾错了会被 Provider 拒绝，但不会被我们静默丢弃）。
     */
    val UNKNOWN: Capability = Capability(TEXT_ONLY, tools = false, reasoning = false, matchedBy = "unknown")
}
