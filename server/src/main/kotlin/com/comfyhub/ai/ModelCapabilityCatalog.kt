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
    const val VERSION = "2026-09"

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

    // 常见的思考档位组合。等级用与我们枚举一致的 off/low/medium/high/max，
    // 值是**线上表达**：字符串=改名，与默认同名时也显式写出来，便于用户看懂。
    private val EFFORT_LOW_MED_HIGH = mapOf("off" to "none", "low" to "low", "medium" to "medium", "high" to "high")
    private val EFFORT_LOW_HIGH = mapOf("off" to "none", "low" to "low", "high" to "high")
    private val EFFORT_HIGH_ONLY = mapOf("off" to "none", "high" to "high")
    private val EFFORT_LOW_MED_HIGH_MAX =
        mapOf("off" to "none", "low" to "low", "medium" to "medium", "high" to "high", "max" to "high")

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
        rule("claude-sonnet-4", TEXT_IMAGE, tools = true)
        rule("claude-opus-4", TEXT_IMAGE, tools = true)
        rule("claude-haiku-4", TEXT_IMAGE, tools = true)
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

        // --- GPT-5 系列：默认就带思考，档位低/中/高 ---
        rule("gpt-5", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)

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
        rule("claude-opus-5", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_HIGH)
        rule("claude-sonnet-5", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_HIGH)
        rule("claude-fable", TEXT_IMAGE, tools = true)
        rule("claude-haiku", TEXT_IMAGE, tools = true)
        rule("claude", TEXT_ONLY, tools = true)
        rule("gemini", TEXT_IMAGE, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH_MAX)
        rule("grok", TEXT_ONLY, tools = true, reasoning = true, efforts = EFFORT_LOW_MED_HIGH)
    }

    /** 查表；返回 null 表示"未知"，调用方必须按保守默认处理。 */
    fun lookup(modelId: String): Capability? {
        val id = modelId.lowercase()
        // 先精确、再包含：宽前缀规则排在后面，避免 `gpt-4` 抢走 `gpt-4o`
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
