package com.comfyhub.ai

import com.comfyhub.AppJson
import kotlinx.serialization.Serializable
import org.slf4j.LoggerFactory
import java.nio.file.Files
import java.nio.file.Path

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
 * 内置模型能力目录 + 可热更新的覆盖层。
 *
 * 需求里明确"不能根据 model id 猜能力"，这条依然成立：内置表**不是猜**，而是一张
 * 按厂商公开文档整理的、带版本的**离线表**，并且：
 *
 *  - 只在接口没有给出能力信息时才使用（接口声明永远优先）；
 *  - 命中的模型在界面上标成「内置目录」，来源可见、可手工改；
 *  - **表里没有的模型一律按"仅文本"处理**，绝不用"名字看着像"去推断图片能力；
 *  - 表可能过期（新模型层出不穷），过期只影响"默认勾选"，用户改一下就覆盖。
 *
 * ## 覆盖层（2026-09-23：让目录能不换构建就更新）
 *
 * 内置表编进 jar，只有发版才能改 —— 新模型层出不穷，这不够。所以 [lookup] 现在
 * 按**逐个维度**合并三层（与 [com.comfyhub.ai.AiUpstream] 的"接口声明 > 目录"同一哲学，
 * 只是目录内部再分两层）：
 *
 * | 层 | 文件 | 谁写 | 定位 |
 * | --- | --- | --- | --- |
 * | custom | `<storage>\ai\capabilities\custom.json` | 用户 / 界面导出 | 手工修正，最高优先 |
 * | remote | `<storage>\ai\capabilities\remote.json` | [com.comfyhub.ai.CapabilityRefresh] 定期拉取 | 在线数据集（models.dev），主力更新源 |
 * | builtin | 代码里的 [rules] | 随版本发布 | 家族级兜底（模糊匹配、含思考档位与方言） |
 *
 * 匹配方式：custom / remote 按**精确 id**（大小写不敏感；网关 id 带 `org/` 前缀而数据集
 * 不带、或反过来时，做**去前缀**匹配，但要求结果唯一 —— 有歧义就宁可不命中）；
 * builtin 沿用**包含匹配**。同一维度三层都有时取上层；某层没给这一维度就落到下一层。
 *
 * 登记四样东西：输入模态、工具、推理（含可选的**思考档位**）、思考方言。
 * 视频 / 音频不做预填：适配器还不支持这些传输方式，勾了也只会被正确阻断。
 */
object ModelCapabilityCatalog {
    private val log = LoggerFactory.getLogger(ModelCapabilityCatalog::class.java)

    const val VERSION = "2026-09b"

    /** 覆盖层文件放 `<storage>\ai\capabilities\` 下（与 skills / memory 同一个 ai 子目录约定）。 */
    const val DIR_NAME = "capabilities"
    const val CUSTOM_FILE = "custom.json"
    const val REMOTE_FILE = "remote.json"

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

    // -----------------------------------------------------------------------
    //  覆盖层：磁盘上的 custom.json / remote.json（@Serializable，条目全维度可选）
    // -----------------------------------------------------------------------

    /** 覆盖层里的一条模型声明。每个维度都可省 —— 省了就落到下一层，**绝不发明默认值**。 */
    @Serializable
    data class OverlayEntry(
        val modalities: List<String>? = null,
        val tools: Boolean? = null,
        val reasoning: Boolean? = null,
        val thinkingEfforts: Map<String, String>? = null,
        val thinkingFormat: String? = null,
    )

    @Serializable
    data class OverlayFile(
        /** 数据集出处（remote 固定 "models.dev"；custom 默认 "custom"），只作展示 */
        val source: String = "custom",
        val version: String? = null,
        /** remote：拉取时间（ISO-8601）；custom 可空 */
        val fetchedAt: String? = null,
        /** key = 模型 id（加载时统一小写） */
        val entries: Map<String, OverlayEntry> = emptyMap(),
    )

    /** 给状态接口 / 设置页看的目录概况。 */
    data class OverlayMeta(val source: String, val fetchedAt: String?, val entryCount: Int)

    data class CatalogStatus(
        val version: String,
        val builtinRules: Int,
        val custom: OverlayMeta?,
        val remote: OverlayMeta?,
    )

    @Volatile private var custom: OverlayFile? = null
    @Volatile private var remote: OverlayFile? = null

    /**
     * 从磁盘重读覆盖层。启动时与每次刷新成功后调用；`null` 清空（测试用）。
     * 文件坏 / 不合法**不抛异常**：记日志、该层当不存在 —— 目录坏了不能让对话也起不来。
     */
    fun reload(dir: Path?) {
        custom = if (dir == null) null else load(dir.resolve(CUSTOM_FILE), "custom")
        remote = if (dir == null) null else load(dir.resolve(REMOTE_FILE), "remote")
    }

    private fun load(file: Path, label: String): OverlayFile? {
        if (!Files.isRegularFile(file)) return null
        return runCatching {
            val parsed = AppJson.decodeFromString(OverlayFile.serializer(), Files.readString(file))
            parsed.copy(entries = parsed.entries.mapKeys { it.key.trim().lowercase() })
        }.onFailure { log.warn("能力覆盖层 {} 读不了（忽略该层）：{} —— {}", label, file, it.message) }
            .getOrNull()
    }

    fun status(): CatalogStatus = CatalogStatus(
        version = VERSION,
        builtinRules = rules.size,
        custom = custom?.let { OverlayMeta(it.source, it.fetchedAt, it.entries.size) },
        remote = remote?.let { OverlayMeta(it.source, it.fetchedAt, it.entries.size) },
    )

    // -----------------------------------------------------------------------
    //  查表：custom > remote > builtin，逐维度合并
    // -----------------------------------------------------------------------

    /** 查表；返回 null 表示"未知"，调用方必须按保守默认处理。 */
    fun lookup(modelId: String): Capability? {
        val id = modelId.lowercase().trim()
        val builtinHit = lookupBuiltin(id)
        val remoteHit = resolveOverlay(remote, id)
        val customHit = resolveOverlay(custom, id)
        if (builtinHit == null && remoteHit == null && customHit == null) return null

        // 逐维度：custom > remote > builtin；某层没给这一维度才落到下一层
        val modalities = customHit?.modalities
            ?: remoteHit?.modalities
            ?: builtinHit?.second?.modalities
            ?: TEXT_ONLY
        val tools = customHit?.tools
            ?: remoteHit?.tools
            ?: builtinHit?.second?.tools
            ?: true
        val reasoning = customHit?.reasoning
            ?: remoteHit?.reasoning
            ?: builtinHit?.second?.reasoning
            ?: false
        // 档位/方言必须与"支持推理"一致（validateThinkingEfforts 的纪律）：
        // 最终 reasoning=false 时一律清空，不管哪层给过
        val efforts = if (!reasoning) emptyMap() else {
            customHit?.thinkingEfforts
                ?: remoteHit?.thinkingEfforts
                ?: builtinHit?.second?.thinkingEfforts
                ?: emptyMap()
        }
        val format = if (!reasoning) null else {
            customHit?.thinkingFormat?.takeIf { it.isNotBlank() }
                ?: remoteHit?.thinkingFormat?.takeIf { it.isNotBlank() }
                ?: builtinHit?.second?.thinkingFormat
        }
        return Capability(
            modalities = modalities,
            tools = tools,
            reasoning = reasoning,
            matchedBy = matchedBy(customHit, remoteHit, builtinHit),
            thinkingEfforts = efforts,
            thinkingFormat = format,
        )
    }

    /** 来源说明（单行，进界面 / capabilityNote）。 */
    private fun matchedBy(
        customHit: OverlayEntry?,
        remoteHit: OverlayEntry?,
        builtinHit: Pair<String, Capability>?,
    ): String = listOfNotNull(
        if (customHit != null) "手工目录 custom.json" else null,
        if (remoteHit != null) "在线目录 ${remote?.source ?: "remote.json"}" else null,
        builtinHit?.let { "内置规则「${it.first}」" },
    ).joinToString(" + ").ifBlank { "unknown" }

    /** builtin 的包含匹配（先命中先用；宽前缀规则排在后面，避免 `gpt-4` 抢走 `gpt-4o`）。 */
    private fun lookupBuiltin(id: String): Pair<String, Capability>? {
        for ((pattern, cap) in rules) {
            if (id.contains(pattern)) return pattern to cap
        }
        return null
    }

    /**
     * 覆盖层匹配：**精确 id** → **去前缀**（网关带 `org/`、数据集不带，或反过来），
     * 去前缀要求**唯一命中** —— 两个条目都叫 `…/dup` 时宁可不命中，绝不猜。
     */
    private fun resolveOverlay(overlay: OverlayFile?, id: String): OverlayEntry? {
        if (overlay == null) return null
        overlay.entries[id]?.let { return it }
        // 数据集带前缀（"z-ai/xxx"）、网关 id 不带（"xxx"）
        val byDatasetSuffix = overlay.entries.filterKeys { it != id && it.substringAfterLast('/') == id }
        if (byDatasetSuffix.size == 1) return byDatasetSuffix.values.first()
        // 网关 id 带前缀（"org/xxx"）、数据集条目不带（"xxx"）
        val bare = id.substringAfterLast('/')
        if (bare != id) {
            val byBare = overlay.entries.filterKeys { it == bare }
            if (byBare.size == 1) return byBare.values.first()
        }
        return null
    }

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
    private val rules: List<Pair<String, Capability>> = buildList {
        fun rule(
            pattern: String,
            modalities: List<String>,
            tools: Boolean,
            reasoning: Boolean = false,
            efforts: Map<String, String> = emptyMap(),
            format: String? = null,
        ) {
            add(
                pattern to Capability(modalities, tools, reasoning, matchedBy = pattern,
                    thinkingEfforts = efforts, thinkingFormat = format),
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
        // DeepSeek 的思考开关是 `thinking{type}`，且**默认开**：不指定强度时会显式发 disabled
        // （界面上的「默认」；网关认不认另说 —— 2026-09-20 实测聚合站全忽略，见 Reasoning.kt）
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

    /**
     * 保守默认：目录里没有、接口也没说 → 仅文本。
     * 图片要用户自己勾（勾错了会被 Provider 拒绝，但不会被我们静默丢弃）。
     *
     * 工具**默认给上**（用户要求）：很多网关不声明工具能力、但实际都支持；
     * 而且当前请求体里根本不发 `tools`，勾选只影响界面显示与预检，不会让请求失败。
     */
    val UNKNOWN: Capability = Capability(TEXT_ONLY, tools = true, reasoning = false, matchedBy = "unknown")
}
