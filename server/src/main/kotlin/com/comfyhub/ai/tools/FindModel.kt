package com.comfyhub.ai.tools

import com.comfyhub.AppJson
import com.comfyhub.ComfyModels
import com.comfyhub.ModelName
import com.comfyhub.ai.CredentialService
import com.comfyhub.ai.OutboundProxy
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import org.slf4j.LoggerFactory
import java.net.URI
import java.net.URLEncoder
import java.net.http.HttpClient
import java.net.http.HttpConnectTimeoutException
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.net.http.HttpTimeoutException
import java.nio.charset.StandardCharsets
import java.time.Duration

/**
 * 「这个模型去哪儿拿」—— `find_model`（用户建议 P3，2026-09-22）的全部逻辑。
 *
 * ## 用户的原话（`docs/bug and suggestion/bug-and-suggestion-9.22.md` 的 A 表与 K 节）
 *
 * > "加入 modelscope、huggingface 的查询功能，并与 civitai 合并成'定向查询' tool；
 * >  **查到的贴链接让用户手动下载**"
 *
 * 决策记录（同文件 D 表第二行）：
 *
 * > | 定向查询 | A：**一条 `find_model`**，内部按 Civitai → HF → ModelScope 顺序查，**逐个报可达性** |
 * > | 能不能代下 | A：**绝不代下**（只给链接 + sha256 + 目标目录）；`download_model` 保持 deny |
 *
 * ## 五条不能含糊的纪律
 *
 *  1. **绝不代下**（用户拍板）。这个文件里**没有任何**"把模型文件写进磁盘"的代码路径：
 *     [HttpModelFinder] 只发**检索**请求，响应体读进内存、取出元数据之后立刻丢掉。
 *     `download_model`（官方 comfy-mcp 那条）在 `McpToolPolicy.DENIED` 里**保持 deny**，
 *     这里一个字都没动它。用户要的是"给我链接，我自己下"。
 *  2. **先查本机，再查外网**（顺序纪律）。同名模型已经在本机时，[ModelFinder.search]
 *     在 [ModelQuery.online] 那一层就返回了 —— **一个外部请求都不发**，
 *     并把三个源标成 [Reachability.SKIPPED]（"本地已有同名，没必要去搜"）。
 *     为什么这么严：2026-09-22 那次事故里，模型让用户去下载一个**本来就在硬盘上**的
 *     4.18GB 文件；只要有一个环节判断错了，这条路就会重演。
 *  3. **每个源都要报可达性**，哪怕它一条都没返回。五个产出状态
 *     （[Reachability.OK] / [NEEDS_PROXY] / [NEEDS_LOGIN] / [GATED] / [ERROR]）
 *     直接对应 A 表那句"工具**必须报可达性**（是否需代理 / 是否 gated / 是否需登录），
 *     否则又变成'AI 说能下、用户下不动'"。
 *  4. **sha256 明写用途**：它是"用户手动下完之后，判断盘上那份是不是同一个模型"的依据。
 *     今天 [ModelName.similarity] 只是**字面**相似（`sd_xl_base_1.0` 与 `sd_xl_refiner_1.0`
 *     能打到 0.63 分却是两份不同的模型），所以字面相似**不足以**确认身份，sha256 才是。
 *     Civitai 用版本 hash、HF 用 LFS oid、ModelScope 用仓库文件的 `Sha256`；
 *     **上游没给就留空并说明**，绝不编一个 64 位十六进制。
 *  5. **只读**。工具声明 `mutating = false`，走 [ToolCategory.WEB]（查询词会离开本机，
 *     与 `web_search` / `civitai_search` 共用同一个联网预算）。
 */
object FindModel {

    /** 一次最多给几条候选（跨源合并之后的数字）。 */
    const val DEFAULT_LIMIT = 8
    const val MAX_LIMIT = 20

    /** 每个源最多问几条（HF 每条还要多打一次目录接口，所以比 [MAX_LIMIT] 小得多）。 */
    const val PER_SOURCE_LIMIT = 4

    const val CONNECT_TIMEOUT_SECONDS = 5L
    const val DEFAULT_TIMEOUT_SECONDS = 20L
    const val MAX_TIMEOUT_SECONDS = 60L

    /**
     * 认得出来的**权重文件**后缀。
     *
     * 与 [ComfyModels] 那张表**故意不一样**：那边是"ComfyUI 能加载什么"（含 `.yaml` / `.json`，
     * 因为 CLIP 那类要配置文件），这边是"用户要手下一个多大的文件" ——
     * 把 `.json` 当成"模型文件"报到候选里只会让人莫名其妙。
     */
    val MODEL_EXTS = setOf("safetensors", "ckpt", "pt", "pth", "bin", "gguf", "sft", "onnx")

    /** HF 令牌在 [CredentialService] 里的引用名（受管凭据库 → `HF_TOKEN` 环境变量兜底）。 */
    const val HF_CREDENTIAL_REF = "huggingface"

    const val HF_BASE_URL = "https://huggingface.co"
    const val MODELSCOPE_BASE_URL = "https://modelscope.cn"

    /** 用户在结果里看到的那句"该放哪"的基准目录（相对路径，绝对路径随机器而变）。 */
    const val INSTALL_ROOT_HINT = "ComfyUI 的 models\\ 目录"
}

/** 三个模型源，声明顺序就是查询顺序（用户拍板：Civitai → HuggingFace → ModelScope）。 */
enum class ModelSource(val wire: String, val label: String) {
    CIVITAI("civitai", "Civitai"),
    HUGGINGFACE("huggingface", "HuggingFace"),
    MODELSCOPE("modelscope", "ModelScope"),
}

/**
 * 一个源**这一问**的可达性。
 *
 * 报的是"这次这一问"的事实，不是"这台机器永久的状态"：同一个源上一问 200、
 * 这一问超时是完全可能的（代理挂了、对方限流）。所以每条都带一句
 * [SourceOutcome.detail]，而不是只给一个枚举值。
 */
enum class Reachability(val wire: String, val label: String) {
    OK("ok", "正常"),
    NEEDS_PROXY("needs_proxy", "本机直连不通，要走出网代理（127.0.0.1:7890）"),
    NEEDS_LOGIN("needs_login", "需要登录 / 令牌"),
    GATED("gated", "需要在网页上同意条款（gated）"),
    ERROR("error", "查询失败"),
    SKIPPED("skipped", "本次没去查（先命中了本机）"),
}

/**
 * 统一后的模型类型 —— "该放进 `models\` 下哪个目录"就靠它推（[ModelCandidate.installInto]）。
 *
 * 认不出来时是 [UNKNOWN]（**不是** `Checkpoint`）：宁可说"不知道放哪"，
 * 也不猜一个目录 —— 猜错的后果是用户把一个 13GB 的 LoRA 下进 `checkpoints\`，
 * 而 ComfyUI 那边照旧看不到它。
 */
enum class ModelKind(val wire: String, val folder: String?, val label: String) {
    CHECKPOINT("checkpoint", "checkpoints", "底模 / Checkpoint"),
    DIFFUSION("diffusion", "diffusion_models", "扩散模型（裸 UNET / DiT）"),
    LORA("lora", "loras", "LoRA / LyCORIS"),
    VAE("vae", "vae", "VAE"),
    CONTROLNET("controlnet", "controlnet", "ControlNet"),
    EMBEDDING("embedding", "embeddings", "Textual Inversion（embedding）"),
    UPSCALER("upscaler", "upscale_models", "放大模型"),
    TEXT_ENCODER("text_encoder", "text_encoders", "文本编码器（CLIP / T5）"),
    CLIP_VISION("clip_vision", "clip_vision", "CLIP Vision"),
    UNKNOWN("unknown", null, "类型认不出来");

    companion object {
        /** Civitai 的 `type` 值（官方拼写，见 `CivitaiSearch.TYPES`）。 */
        fun fromCivitai(raw: String?): ModelKind = when (raw?.trim()?.lowercase()) {
            "checkpoint" -> CHECKPOINT
            "lora", "locon", "dora" -> LORA
            "vae" -> VAE
            "controlnet" -> CONTROLNET
            "textualinversion" -> EMBEDDING
            "upscaler" -> UPSCALER
            else -> UNKNOWN
        }

        /**
         * HF 的 `pipeline_tag` / ModelScope 的 `Tasks`。
         *
         * 只映射**确定**的那几种：这两个源给的是"这个仓库是干什么的"，
         * 不是 ComfyUI 那种精确的加载器类型。`text-to-image` 落
         * [DIFFUSION]（实际上更可能是整包 checkpoint）—— 所以工具在渲染时
         * **额外**看仓库路径里有没有 `lora` / `vae` / `diffusion_models` 这类线索
         * （见 [fromPathHint]），那比 pipeline_tag 准。
         */
        fun fromTask(raw: String?): ModelKind = when (raw?.trim()?.lowercase()) {
            "text-to-image", "image-to-image", "unconditional-image-generation",
            "text-to-video", "image-to-video", "any-to-any", "diffusers",
            -> DIFFUSION
            "lora", "peft" -> LORA
            else -> UNKNOWN
        }

        /**
         * 从仓库名 / 文件路径里的目录名反推该放哪（HF 与 ModelScope 的兜底）。
         *
         * 判据是**路径里真的出现了那个目录名**（`split_files/diffusion_models/xxx.safetensors`、
         * `.../loras/...`），不是猜 —— 认不出来就返回 null，让报告说"放哪说不准"。
         *
         * ⚠ **仓库名本身也算路径的一段**（调用方把 `repoId + "/" + 文件名` 传进来）。
         * 实测（2026-09-22）：`Omnico/Krea2_turbo_diff_loras` 这个仓库里的文件叫
         * `Moody5_r256.safetensors` —— 光看文件名只能推 ?，而它明明写着 loras。
         */
        fun fromPathHint(path: String?): ModelKind? {
            val segments = path.orEmpty().replace('\\', '/').split('/').map { it.lowercase() }
            return when {
                // `loras` / `lora` 整段，或 `…_loras` / `lora-…` 这种被夹在名字里的写法
                segments.any { s -> s.split('-', '_', '.').any { it == "loras" || it == "lora" } } -> LORA
                segments.any { it == "diffusion_models" || it == "unet" || it == "dit" } -> DIFFUSION
                segments.any { it == "vae" } -> VAE
                segments.any { it == "controlnet" } -> CONTROLNET
                segments.any { it == "embeddings" } -> EMBEDDING
                segments.any { it == "upscale_models" } -> UPSCALER
                segments.any { it == "text_encoders" } -> TEXT_ENCODER
                segments.any { it == "clip_vision" } -> CLIP_VISION
                segments.any { it == "checkpoints" } -> CHECKPOINT
                else -> null
            }
        }

        /**
         * 从**文件名本身**反推（比仓库名更硬）。
         *
         * 现场（2026-09-22 实测）：`Beinsezii/Krea-2-Turbo-Projector-Scale-LoRA-Diffusers`
         * 里那个 `<1MB` 的 `pytorch_lora_weights.safetensors` 其实就是个 LoRA ——
         * 它的文件名是 HuggingFace diffusers 训练脚本的固定产物名，不是模型作者起的。
         */
        fun fromFileName(fileName: String?): ModelKind? {
            val base = fileName.orEmpty().replace('\\', '/').substringAfterLast('/').lowercase()
            return when {
                base.contains("lora") -> LORA
                base.contains("controlnet") || base.contains("control_net") -> CONTROLNET
                base.contains("vae") || base == "ae.safetensors" -> VAE
                base.contains("clip_vision") -> CLIP_VISION
                base.contains("text_encoder") -> TEXT_ENCODER
                base.contains("embedding") -> EMBEDDING
                else -> null
            }
        }

        /**
         * 把三条线索按可信度排一遍：**文件名 → 路径/仓库名 → 源的 pipeline_tag / Tasks**。
         *
         * 顺序不能反：`pipeline_tag=text-to-image` 说的是"这个仓库干什么用"，
         * 它**说不出**"该放进 ComfyUI 的哪个目录"（diffusion / checkpoint / LoRA 都可能）。
         */
        fun infer(fileName: String?, path: String?, task: String?): ModelKind =
            fromFileName(fileName) ?: fromPathHint(path) ?: fromTask(task)

        /**
         * 这个仓库是不是 **diffusers 目录格式**（一堆子目录 + `model_index.json`）。
         *
         * 判据来自仓库自己声明的 `library_name` / `tags`（HF 检索接口里就有），
         * 不是我们猜的。为什么要单列：diffusers 格式**没有**"一个能放进
         * `diffusion_models\` 的 safetensors" —— 里面的 `transformer/`、`vae/` 是分片的，
         * 而且都是**文件夹**，拿单个权重文件去 `UNETLoader` 里填很可能跑不起来。
         * 这条如实说明比给一个"看起来能用"的 `installInto` 有用得多。
         */
        fun isDiffusersFormat(libraryName: String?, tags: List<String>): Boolean =
            libraryName?.equals("diffusers", ignoreCase = true) == true ||
                tags.any { it.equals("diffusers", ignoreCase = true) }
    }
}

/**
 * 统一后的一个候选。
 *
 * 字段就是 A 表第 9 条点名要能对齐的那几个：
 * 名字 / 类型 / 底模 / 触发词 / 体积 / 直链 / **sha256** / **该放哪个目录** / 来源。
 */
data class ModelCandidate(
    val name: String,
    val kind: ModelKind,
    /** 底模（Civitai 的 `baseModel` / HF 的 `base_model:*` 标签 / ModelScope 的 `BaseModel`）。 */
    val baseModel: String? = null,
    val trainedWords: List<String> = emptyList(),
    val sizeBytes: Long? = null,
    /** 直链（**给用户自己下**用的那个 URL）。 */
    val downloadUrl: String? = null,
    /** 模型页（简介 / 许可 / 触发词在那边）。 */
    val pageUrl: String? = null,
    /**
     * sha256（十六进制）。
     *
     * 第二用途写在 [FindModel] 类注释第 4 条：用户手动下完之后拿它比对盘上那份，
     * 判断"是不是同一个模型"——**字面相似度做不到这件事**。上游没给就是 null。
     */
    val sha256: String? = null,
    /** 文件名（有的话；它 + sha256 才够确认"下到的就是它"）。 */
    val fileName: String? = null,
    /** 这一条的主来源。 */
    val source: ModelSource,
    /** 这条候选在哪些源上出现过（去重合并之后的并集，顺序与查询顺序一致）。 */
    val sources: List<ModelSource> = listOf(source),
    /** 简介（只有 Civitai 会给；别的源留 null —— 不编）。 */
    val description: String? = null,
    val downloads: Int? = null,
    /**
     * 一句**必须转告用户**的提醒（拿不到类型 / 格式不对口时才有）。
     *
     * 现场（2026-09-22 实测）：HF 上 `krea/Krea-2-Turbo` 最大那个文件是
     * `turbo.safetensors`（24.48GB），而那个仓库是 **diffusers 目录格式** ——
     * 直接下这一个文件丢进 `diffusion_models\` 很可能跑不起来。这时 `installInto` 为 null，
     * 这一句会连同候选一起给出来，而不是让模型自己编一个目录。
     */
    val note: String? = null,
) {
    /**
     * **它该放进哪个目录**（相对用户 ComfyUI 的 `models\`）。
     *
     * 拿不到类型时是 null：工具会明写"该放 `models\` 下哪个子目录说不准，
     * 让用户自己看工作流里那个加载器"——这比猜一个好。
     */
    val installInto: String? get() = kind.folder

    /**
     * 给人看的体积（与 `CivitaiModel.sizeLabel()` 同一套分档：小的用 MB、大的用 GB）。
     *
     * `%.1f` 而不是 `%.0f`：实测有个 `pytorch_lora_weights.safetensors` 只有几 MB，
     * 用整数会显示成 **"0 MB"** —— 那看起来像"空文件"，用户会以为拿不到东西。
     */
    fun sizeLabel(): String? = sizeBytes?.let {
        val gb = it / 1024.0 / 1024.0 / 1024.0
        if (gb >= 1.0) "%.2f GB".format(gb) else "%.1f MB".format(it / 1024.0 / 1024.0)
    }
}

/**
 * 一个源的产出。
 *
 * **失败也要有位置** —— 这正是"逐个报可达性"那条纪律的实现方式：不可达的源
 * 在结果里照样占一行，写明状态与原因，而不是从列表里消失。
 */
sealed interface SourceOutcome {
    val source: ModelSource
    val state: Reachability

    /** 给人看的一句话（写进结果里；**绝不回显令牌**）。 */
    val detail: String

    data class Ok(
        override val source: ModelSource,
        val candidates: List<ModelCandidate>,
        override val detail: String = "",
    ) : SourceOutcome {
        override val state: Reachability get() = Reachability.OK
    }

    data class Failed(
        override val source: ModelSource,
        override val state: Reachability,
        override val detail: String,
    ) : SourceOutcome
}

/** 一次查询的入参（工具层填好之后交给 [ModelFinder]）。 */
data class ModelQuery(
    val text: String,
    /**
     * 期望的模型类型（工具据此再写一句"该放哪"）；null = 不限。
     *
     * 它**不改变查询词**：三个源的检索接口都只吃关键词。这一位的用途是
     * 在候选的类型认不出来时兜底说"用户要的是 LoRA，所以大概率进 `loras\`"。
     */
    val kind: ModelKind? = null,
    val limit: Int = FindModel.DEFAULT_LIMIT,
    /**
     * 要不要出网。
     *
     * `false` = **本机优先那条路的产物**：调用方（或 [ModelFinder] 自己）已经在本机
     * 清单里命中了同名模型，不需要、也不该再往外发请求。
     */
    val online: Boolean = true,
    /**
     * 本机清单（`/object_info` ∪ 盘扫）。
     *
     * 刻意由调用方传进来、**不在这里读环境**（与 [ComfyModels.inventory] 同一个理由）：
     * 单测才不会被开发机的真实 ComfyUI 目录影响。
     */
    val inventory: ComfyModels.Inventory? = null,
)

/** 一次查询的结果（**三个源一条不少**，见 [SourceOutcome]）。 */
data class ModelFinderResult(
    val query: String,
    /** 三个源的产出，顺序固定 = [ModelSource.entries]。 */
    val sources: List<SourceOutcome>,
    /** 合并去重后的候选（跨源同名的合一条，[ModelCandidate.sources] 列全）。 */
    val candidates: List<ModelCandidate>,
    /** 本机命中的那些（本地短路的依据）。 */
    val localMatches: List<ComfyModels.LocalModel>,
    /** 本机"名字沾边但不是一个东西"的那些 —— 用来挡"我明明有这个模型"那类误判。 */
    val localNearMisses: List<String> = emptyList(),
    /** 本机清单的来源（`object_info` / `scan` / `none`），报告里要能说清"本机清单是哪来的"。 */
    val localSource: ComfyModels.Source = ComfyModels.Source.NONE,
    /** 这次到底出没出网（本地短路时为 false）。 */
    val online: Boolean = true,
    val notes: List<String> = emptyList(),
) {
    /** 本机已经有同名的了 → 工具会照着这条说"不用去下"。 */
    val localHit: Boolean get() = localMatches.isNotEmpty()
}

/**
 * [ToolRegistry] 依赖的**窄接口**：工具层只认"查一次"这一个动作。
 *
 * 与 [CivitaiClient] / [WebSearchClient] 同一个理由：`ToolRegistryTest` 要能塞一个
 * **不起 HTTP 的假实现**去钉"本机优先会短路 / 三源都要报状态 / 字段齐不齐"；
 * 真正打 HTTP 的部分（三家的 URL、鉴权头、错误映射、sha256 从哪个字段取）
 * 由 `FindModelTest` 拿本机假服务钉死。
 *
 * ⚠ 这个接口**刻意没有任何"取文件"的入口** —— "绝不代下"这条纪律因此在形状上就成立，
 * 而不是靠自觉：能调用的动作只有"搜一次"。
 */
interface ModelFinder {
    suspend fun search(query: ModelQuery): ModelFinderResult
}

/** 各家真实的检索实现（会打 HTTP，全部走 [OutboundProxy]）。 */
class HttpModelFinder(
    /**
     * Civitai 的客户端**直接复用**已经上线的那一个（[CivitaiSearch]）——
     * 域名切换 / 令牌 / 错误映射 / 代理选择器都在它里面，这里一行都不重复实现。
     */
    private val civitai: CivitaiClient? = null,
    /** 令牌（HF gated / 私有仓库要用）；拿不到就是匿名。**绝不写进日志或结果**。 */
    private val credentials: CredentialService? = null,
    hfBaseUrl: String = defaultHfBaseUrl(),
    modelscopeBaseUrl: String = defaultModelScopeBaseUrl(),
    private val timeoutSeconds: Long = FindModel.DEFAULT_TIMEOUT_SECONDS,
    /** 每个源最多问几条（[FindModel.PER_SOURCE_LIMIT]）。 */
    private val perSource: Int = FindModel.PER_SOURCE_LIMIT,
) : ModelFinder {

    private val log = LoggerFactory.getLogger(HttpModelFinder::class.java)

    private val hfBase = hfBaseUrl.trim().trimEnd('/')
    private val msBase = modelscopeBaseUrl.trim().trimEnd('/')

    private val client: HttpClient = HttpClient.newBuilder()
        .connectTimeout(Duration.ofSeconds(FindModel.CONNECT_TIMEOUT_SECONDS))
        // 与 CivitaiSearch / KeenableSearch 同规矩：目标是我们写死的域名，不跟随重定向
        .followRedirects(HttpClient.Redirect.NEVER)
        // 本机的代理设置要认 —— JVM 默认一律直连，而本机 civitai / HF 直连不通（见 OutboundProxy）
        .proxy(OutboundProxy.selector())
        .build()

    companion object {
        private fun defaultHfBaseUrl(): String =
            System.getenv("COMFYHUB_HF_BASE_URL")?.takeIf { it.isNotBlank() } ?: FindModel.HF_BASE_URL

        private fun defaultModelScopeBaseUrl(): String =
            System.getenv("COMFYHUB_MODELSCOPE_BASE_URL")?.takeIf { it.isNotBlank() }
                ?: FindModel.MODELSCOPE_BASE_URL
    }

    override suspend fun search(query: ModelQuery): ModelFinderResult {
        val text = query.text.trim()
        if (text.isEmpty()) throw ToolFailure("INVALID_ARGUMENT", "缺少搜索词 query")

        // ① 先查本机（用户拍板的顺序纪律）。命中了就**一个外部请求都不发**。
        val hit = localHit(text, query.inventory)
        if (!query.online || hit != null) {
            val local = listOfNotNull(hit)
            return ModelFinderResult(
                query = text,
                sources = ModelSource.entries.map { skipped(it, local.isNotEmpty()) },
                candidates = emptyList(),
                localMatches = local,
                localNearMisses = nearMisses(text, query, hit),
                localSource = query.inventory?.source ?: ComfyModels.Source.NONE,
                online = false,
                notes = if (local.isNotEmpty()) {
                    listOf("本机已有同名文件，**没有**去外网搜索（先查本机、再查外网）。")
                } else {
                    listOf("本次只查了本机（调用方关掉了出网）。")
                },
            )
        }

        // ② 三个源按固定顺序查；**单个源失败不影响其它源**（"逐个报可达性"的前提）
        val outcomes = listOf(
            civitai(text, query.limit),
            huggingFace(text),
            modelScope(text),
        )
        val merged = merge(outcomes.filterIsInstance<SourceOutcome.Ok>().flatMap { it.candidates })
            .take(query.limit.coerceIn(1, FindModel.MAX_LIMIT))

        val notes = mutableListOf<String>()
        if (merged.isEmpty() && outcomes.none { it.state == Reachability.OK }) {
            notes += "三个源这次都没查到东西（原因见每个源那一行的状态）——" +
                "如实告诉用户，不要编模型名。"
        }
        if (merged.isNotEmpty() && merged.none { it.sha256 != null }) {
            notes += "本次没有一条带 sha256（上游没给）——那几份下载完**没法用 sha256 校验**，" +
                "如实说明这一点，不要拿字面相似度当「同一个模型」的证据。"
        }

        return ModelFinderResult(
            query = text,
            sources = outcomes,
            candidates = merged,
            localMatches = emptyList(),
            localNearMisses = nearMisses(text, query, null),
            localSource = query.inventory?.source ?: ComfyModels.Source.NONE,
            online = true,
            notes = notes,
        )
    }

    /** 本地短路时，每个源都要在结果里占一行，写清"为什么没查"。 */
    private fun skipped(source: ModelSource, local: Boolean): SourceOutcome.Failed = SourceOutcome.Failed(
        source = source,
        state = Reachability.SKIPPED,
        detail = if (local) {
            "没查：本机已有同名模型（用户拍板的顺序是先查本机、再查外网）"
        } else {
            "没查：本次只查本机"
        },
    )

    /** 缺模型时把"沾边但不够像"的本机文件报出来（用户建议 ②：那是"我明明有"误判的来源）。 */
    private fun nearMisses(
        text: String,
        query: ModelQuery,
        hit: ComfyModels.LocalModel?,
    ): List<String> {
        val inv = query.inventory ?: return emptyList()
        val folder = hit?.folder ?: query.kind?.folder ?: return emptyList()
        return inv.nearMissesFor(folder, text).map { it.name }
    }

    /**
     * 本机有没有同名的。
     *
     * 两层判据，**都不用纯子串**：`anima` 会命中 `anima-turbo`，而那是**另一个模型** ——
     * 短路到"本机有、不用下"的代价，是用户手里少一个真需要的模型。
     *  ① [ModelName.same]（分词后完全一致：`krea2_turbo_int8` 与 `krea2TurboInt8` 是同一个）；
     *  ② 文件名（去掉目录前缀与后缀）**完全相等** —— 用来认 `/object_info` 那种
     *     `SDXL\a.safetensors` 的写法与带后缀 / 不带后缀的差别。
     */
    private fun localHit(text: String, inventory: ComfyModels.Inventory?): ComfyModels.LocalModel? {
        val inv = inventory ?: return null
        val want = text.trim()
        if (want.isEmpty()) return null
        val wantBase = want.replace('\\', '/').substringAfterLast('/').substringBeforeLast('.')
        return inv.models.firstOrNull { m ->
            val base = m.name.replace('\\', '/').substringAfterLast('/').substringBeforeLast('.')
            ModelName.same(want, m.name) || base.equals(wantBase, ignoreCase = true)
        }
    }

    // -----------------------------------------------------------------------
    //  Civitai（复用 CivitaiSearch）
    // -----------------------------------------------------------------------

    private suspend fun civitai(text: String, limit: Int): SourceOutcome {
        val search = civitai
            ?: return SourceOutcome.Failed(
                ModelSource.CIVITAI,
                Reachability.ERROR,
                "本次运行没有接入 Civitai 搜索客户端（CIVITAI_DISABLED）",
            )
        return try {
            // 固定 `Most Downloaded` + `AllTime`（CivitaiSearch 内部定的）：用户要的是
            // "这个模型的正主在哪儿"，而不是"最近谁传了个同名的"。
            //
            // `allowNsfw` **不在这里打开**：`find_model` 的语义是"帮我找某个具体的模型 / 文件"，
            // 成人内容有 `civitai_search` 那条专线（它才带 allowNsfw）。这里打开它，
            // 等于替用户做了一个他没做的决定。
            val outcome = search.search(
                query = text,
                allowNsfw = false,
                type = null,
                limit = limit.coerceIn(1, FindModel.PER_SOURCE_LIMIT),
            )
            val candidates = outcome.models.map { m ->
                ModelCandidate(
                    name = m.name,
                    kind = ModelKind.fromCivitai(m.type),
                    baseModel = m.baseModel.takeIf { it.isNotBlank() },
                    trainedWords = m.trainedWords,
                    sizeBytes = m.sizeBytes,
                    downloadUrl = m.downloadUrl,
                    pageUrl = m.pageUrl,
                    // 版本 hash（线上 `modelVersions[0].files[primary].hashes.SHA256`）
                    sha256 = m.sha256,
                    fileName = m.fileName,
                    source = ModelSource.CIVITAI,
                    description = m.description.takeIf { it.isNotBlank() },
                    downloads = m.downloads,
                )
            }
            SourceOutcome.Ok(
                source = ModelSource.CIVITAI,
                candidates = candidates,
                detail = if (candidates.isEmpty()) "通了，但这一问没有结果" else "通了，${candidates.size} 条",
            )
        } catch (e: ToolFailure) {
            failedFromFailure(ModelSource.CIVITAI, e)
        } catch (e: Exception) {
            // 连"出网"都没走到就炸了（解析 / 本地 bug）：如实归到 error，别赖到代理头上
            SourceOutcome.Failed(
                ModelSource.CIVITAI,
                Reachability.ERROR,
                "查询失败（${e::class.simpleName}）：这是本机的处理出了问题，不是 Civitai 的锅",
            )
        }
    }

    // -----------------------------------------------------------------------
    //  HuggingFace
    // -----------------------------------------------------------------------

    private suspend fun huggingFace(text: String): SourceOutcome {
        val token = hfToken()
        return try {
            val searchUrl = "$hfBase/api/models?" + listOf(
                "search=" + enc(text),
                "limit=" + perSource.coerceIn(1, FindModel.PER_SOURCE_LIMIT),
                "sort=downloads",
                "direction=-1",
            ).joinToString("&")
            // ⚠ `GET /api/models` 返回的是一个 **JSON 数组**（不是对象）——
            // 拿 `request()`（它假定顶层是对象）去取会得到"无法解析的内容"，
            // 而那看起来像"HF 挂了"。数组走 getJsonArray，对象走 request。
            val repos = getJsonArray(searchUrl, ModelSource.HUGGINGFACE, token)
            val candidates = repos.mapNotNull { el ->
                runCatching { el.jsonObject }.getOrNull()?.let { hfCandidate(it, token) }
            }
            SourceOutcome.Ok(
                source = ModelSource.HUGGINGFACE,
                candidates = candidates,
                detail = if (candidates.isEmpty()) "通了，但这一问没有结果" else "通了，${candidates.size} 条",
            )
        } catch (e: SourceHttpException) {
            SourceOutcome.Failed(ModelSource.HUGGINGFACE, e.state, e.detail)
        } catch (e: Exception) {
            SourceOutcome.Failed(
                ModelSource.HUGGINGFACE,
                Reachability.ERROR,
                "查询失败（${e::class.simpleName}）",
            )
        }
    }

    /**
     * 一个 HF 仓库 → 一条候选。
     *
     * 要**第二次**请求（`/api/models/{repo}/tree/main`）才拿得到文件名与 LFS oid。
     * 那次失败是**吞掉**的（降级成"没有文件名 / 没有 sha256"）而不是把整个源判成失败：
     * 一个仓库的目录读不到，不该让"HF 这个源可达"这个事实变成"HF 挂了"。
     */
    private suspend fun hfCandidate(repo: JsonObject, token: String?): ModelCandidate? {
        val repoId = repo["modelId"]?.jsonPrimitive?.contentOrNull
            ?: repo["id"]?.jsonPrimitive?.contentOrNull
            ?: return null
        val tags = repo["tags"]?.let { runCatching { it.jsonArray }.getOrNull() }.orEmpty()
            .mapNotNull { runCatching { it.jsonPrimitive.contentOrNull }.getOrNull() }
        val baseModel = tags.firstOrNull { it.startsWith("base_model:", ignoreCase = true) }
            ?.substringAfter("base_model:")

        val files = runCatching { hfTree(repoId, token) }.getOrElse { emptyList() }
        val best = files.maxByOrNull { it.size ?: 0L }

        // 类型：**文件名 → 仓库名/路径 → pipeline_tag**（后者的信息量最小，见 ModelKind.infer）。
        // 仓库名也传进去：`Omnico/Krea2_turbo_diff_loras` 这种线索只在仓库名里。
        val kind = ModelKind.infer(best?.path, "$repoId/${best?.path.orEmpty()}", repo["pipeline_tag"]?.jsonPrimitive?.contentOrNull)

        // diffusers 目录格式：**没有**"一个能放进 diffusion_models\ 的文件"。
        // 这时宁可 `installInto = null` + 一句说明，也不给一个看着能用的目录
        // （实测：`krea/Krea-2-Turbo` 最大那个文件是分片目录里的 `turbo.safetensors`，24.48GB）。
        val diffusers = ModelKind.isDiffusersFormat(
            repo["library_name"]?.jsonPrimitive?.contentOrNull,
            tags,
        )
        val effectiveKind = if (diffusers && kind in setOf(ModelKind.DIFFUSION, ModelKind.CHECKPOINT)) {
            ModelKind.UNKNOWN
        } else {
            kind
        }
        val note = if (diffusers && effectiveKind == ModelKind.UNKNOWN) {
            "这个仓库是 **diffusers 目录格式**（`model_index.json` + 一堆子目录），" +
                "里面**没有**一个能直接放进 ComfyUI 的整包 safetensors —— " +
                "去模型页找作者提供的**单文件**版本（通常在同一个仓库的 Files 里，或另一个 repo），" +
                "或者让用户自己转格式。**别把这个文件当成能直接用的底模。**"
        } else {
            null
        }

        if (best == null) {
            // 一个权重文件都没有（只有 README / config）：**仍然报出来** ——
            // 用户要找的可能正是"这个仓库到底有没有 safetensors"。但体积 / sha256 只能是空。
            return ModelCandidate(
                name = repoId,
                kind = effectiveKind,
                baseModel = baseModel,
                downloadUrl = fileLink(repoId, null),
                pageUrl = "${FindModel.HF_BASE_URL}/$repoId",
                source = ModelSource.HUGGINGFACE,
                downloads = repo["downloads"]?.jsonPrimitive?.intOrNull,
                note = note,
            )
        }
        return ModelCandidate(
            name = repoId,
            kind = effectiveKind,
            baseModel = baseModel,
            sizeBytes = best.size,
            downloadUrl = fileLink(repoId, best.path),
            pageUrl = "${FindModel.HF_BASE_URL}/$repoId",
            // HF 的 sha256 **就是 LFS oid**（`lfs.oid` 正是那份大文件的 sha256）。
            // 非 LFS 文件（走 git 对象的小文件）没有 `lfs` 字段 → 留 null 并如实说明。
            sha256 = best.sha256,
            fileName = best.path,
            source = ModelSource.HUGGINGFACE,
            downloads = repo["downloads"]?.jsonPrimitive?.intOrNull,
            note = note,
        )
    }

    /** 直链给**正式站**，不是本次用的 base（base 可以被环境变量指到本机假服务）。 */
    private fun fileLink(repoId: String, path: String?): String =
        if (path == null) "${FindModel.HF_BASE_URL}/$repoId"
        else "${FindModel.HF_BASE_URL}/$repoId/resolve/main/$path"

    private data class WeightFile(val path: String, val size: Long?, val sha256: String?)

    /** `/api/models/{repo}/tree/main`：抓**最大**的那个权重文件（用户要下的就是它）。 */
    private suspend fun hfTree(repoId: String, token: String?): List<WeightFile> {
        val url = "$hfBase/api/models/$repoId/tree/main"
        // 同样是 **JSON 数组**（见 huggingFace 里那条注释）
        val arr = runCatching { getJsonArray(url, ModelSource.HUGGINGFACE, token) }.getOrNull()
            ?: return emptyList()
        return arr.mapNotNull { el ->
            val o = runCatching { el.jsonObject }.getOrNull() ?: return@mapNotNull null
            val type = o["type"]?.jsonPrimitive?.contentOrNull
            if (type != null && type != "file") return@mapNotNull null
            val path = o["path"]?.jsonPrimitive?.contentOrNull ?: return@mapNotNull null
            if (!looksLikeWeight(path)) return@mapNotNull null
            val oid = o["lfs"]?.let { runCatching { it.jsonObject }.getOrNull() }
                ?.get("oid")?.jsonPrimitive?.contentOrNull
            WeightFile(
                path = path,
                size = o["size"]?.jsonPrimitive?.asLong(),
                // oid 只有 64 位十六进制才是 sha256（上游结构变了也不会误当成 hash 报出去）
                sha256 = oid?.takeIf { isSha256(it) },
            )
        }
    }

    /** HF 令牌：受管凭据库优先、`HF_TOKEN` 环境变量兜底（与 CivitaiSearch 的两路同一约定）。 */
    private fun hfToken(): String? {
        runCatching { credentials?.resolve(FindModel.HF_CREDENTIAL_REF) }.getOrNull()
            ?.takeIf { it.isNotBlank() }
            ?.let { return it }
        return System.getenv("HF_TOKEN")?.takeIf { it.isNotBlank() }
    }

    // -----------------------------------------------------------------------
    //  ModelScope
    // -----------------------------------------------------------------------

    private suspend fun modelScope(text: String): SourceOutcome {
        return try {
            // ⚠ 线上核对过（2026-09-22 本机实测）：`GET /api/v1/models?...` 是 **404**，
            // 检索接口是 `PUT /api/v1/dolphin/models` + JSON 体（魔搭前端自己走的那条）。
            // 这不是猜的 —— 404 与 200 各打了一遍才写成这样。
            val body = buildJsonObject {
                put("PageSize", perSource.coerceIn(1, FindModel.PER_SOURCE_LIMIT))
                put("PageNumber", 1)
                put("Name", text)
                put("SortBy", "Default")
            }
            val json = request(
                "$msBase/api/v1/dolphin/models",
                source = ModelSource.MODELSCOPE,
                body = body,
                // **PUT**，不是 POST/GET：实测同一路径 PUT 200、GET 与 POST 都是 404
                method = "PUT",
            )
            val candidates = msModels(json).mapNotNull { modelScopeCandidate(it) }
            SourceOutcome.Ok(
                source = ModelSource.MODELSCOPE,
                candidates = candidates,
                detail = if (candidates.isEmpty()) "通了，但这一问没有结果" else "通了，${candidates.size} 条",
            )
        } catch (e: SourceHttpException) {
            SourceOutcome.Failed(ModelSource.MODELSCOPE, e.state, e.detail)
        } catch (e: Exception) {
            SourceOutcome.Failed(ModelSource.MODELSCOPE, Reachability.ERROR, "查询失败（${e::class.simpleName}）")
        }
    }

    /** 从 `{"Data":{"Model":{"Models":[…]}}}` 里挖出模型数组（结构在线上核对过）。 */
    private fun msModels(json: JsonObject): List<JsonObject> {
        val data = json["Data"]?.let { runCatching { it.jsonObject }.getOrNull() }
        val model = data?.get("Model")?.let { runCatching { it.jsonObject }.getOrNull() }
        val arr: JsonArray? = (model?.get("Models") ?: data?.get("Models") ?: json["Models"])
            ?.let { runCatching { it.jsonArray }.getOrNull() }
        return arr.orEmpty().mapNotNull { runCatching { it.jsonObject }.getOrNull() }
    }

    private suspend fun modelScopeCandidate(model: JsonObject): ModelCandidate? {
        val path = model["Path"]?.jsonPrimitive?.contentOrNull?.takeIf { it.isNotBlank() } ?: return null
        val name = model["Name"]?.jsonPrimitive?.contentOrNull?.takeIf { it.isNotBlank() } ?: path
        // ⚠ 仓库全名是 **`Path` + `/` + `Name`** —— 实测（2026-09-22，带代理打了两次）：
        // `Path="krea"` + `Name="Krea-2-Turbo"` → `krea/Krea-2-Turbo` 的 `repo/files` 返回 200，
        // 而只拿 `Path`（`/api/v1/models/krea/repo/files`）是 **404** ——
        // 那个 404 会让每个候选都丢掉"文件名 / 直链 / sha256"，看起来像"魔搭没这些数据"。
        // `Path` 里已经带斜杠时（`circlestone-labs/Anima`）就是全名，不要再拼一次。
        val repo = if (path.contains('/')) path else "$path/$name"
        val tasks = model["Tasks"]?.let { runCatching { it.jsonArray }.getOrNull() }.orEmpty()
            .mapNotNull { runCatching { it.jsonPrimitive.contentOrNull }.getOrNull() }
        val baseModel = model["BaseModel"]?.let { runCatching { it.jsonArray }.getOrNull() }.orEmpty()
            .mapNotNull { runCatching { it.jsonPrimitive.contentOrNull }.getOrNull() }
            .firstOrNull()

        // 体积：`ModelInfos` 里每个框架一份 `{model_size: …}`。取**最大**那个 ——
        // 用户要下的是权重本体，不是 tokenizer 那几 MB。
        val metaSize = model["ModelInfos"]?.let { runCatching { it.jsonObject }.getOrNull() }?.values.orEmpty()
            .mapNotNull { info ->
                runCatching { info.jsonObject }.getOrNull()
                    ?.get("model_size")?.jsonPrimitive?.asLong()
            }
            .maxOrNull()

        // sha256 + 文件名：目录接口 `repo/files` 里每个 blob 带 `Sha256`
        // （线上核对过：`.gitattributes` 那种小文件也有）。取最大的那个权重文件。
        val file = runCatching { msLargestFile(repo) }.getOrElse { null }
        return ModelCandidate(
            name = name,
            // 三条线索按可信度排：文件名 → 路径 + 仓库名 → Tasks（见 ModelKind.infer）
            kind = ModelKind.infer(file?.path, "$repo/${file?.path.orEmpty()}", tasks.firstOrNull()),
            baseModel = baseModel,
            sizeBytes = file?.size ?: metaSize,
            downloadUrl = file?.path
                ?.let { "${FindModel.MODELSCOPE_BASE_URL}/models/$repo/resolve/master/$it" },
            pageUrl = "${FindModel.MODELSCOPE_BASE_URL}/models/$repo",
            sha256 = file?.sha256,
            fileName = file?.path,
            source = ModelSource.MODELSCOPE,
            downloads = model["Downloads"]?.jsonPrimitive?.intOrNull,
        )
    }

    private suspend fun msLargestFile(path: String): WeightFile? {
        val url = "$msBase/api/v1/models/$path/repo/files?Revision=master"
        val arr = runCatching {
            request(url, source = ModelSource.MODELSCOPE, method = "GET")["Data"]
                ?.let { runCatching { it.jsonObject }.getOrNull() }
                ?.get("Files")?.jsonArray
        }.getOrNull() ?: return null
        return arr.mapNotNull { el ->
            val o = runCatching { el.jsonObject }.getOrNull() ?: return@mapNotNull null
            if (o["Type"]?.jsonPrimitive?.contentOrNull != "blob") return@mapNotNull null
            val p = o["Path"]?.jsonPrimitive?.contentOrNull ?: return@mapNotNull null
            if (!looksLikeWeight(p)) return@mapNotNull null
            WeightFile(
                path = p,
                size = o["Size"]?.jsonPrimitive?.asLong(),
                sha256 = o["Sha256"]?.jsonPrimitive?.contentOrNull?.takeIf { isSha256(it) },
            )
        }.maxByOrNull { it.size ?: 0L }
    }

    // -----------------------------------------------------------------------
    //  合并去重
    // -----------------------------------------------------------------------

    /**
     * 跨源同名的合成一条。
     *
     * 键是 `分词集合 + 归一化文件名`：`krea2_turbo_int8.safetensors` 与
     * `krea2-turbo-int8.safetensors`（两个源上的同一份东西）必须合成一条 ——
     * 否则用户会看到"两个不同的模型"，而它们其实是同一个。
     *
     * 合并时**优先保留信息更全的那条**（有 sha256 / 体积 / 触发词的赢），
     * 并把两个来源都记进 [ModelCandidate.sources]。
     */
    private fun merge(all: List<ModelCandidate>): List<ModelCandidate> {
        val byKey = LinkedHashMap<String, ModelCandidate>()
        all.forEach { c ->
            val key = ModelName.tokens(c.name).sorted().joinToString("-") + "|" +
                (c.fileName ?: "").substringAfterLast('/').lowercase()
            val prev = byKey[key]
            byKey[key] = if (prev == null) c else richer(prev, c)
        }
        return byKey.values.toList()
    }

    private fun richer(a: ModelCandidate, b: ModelCandidate): ModelCandidate {
        val primary = if (score(b) > score(a)) b else a
        val other = if (primary === a) b else a
        return primary.copy(
            trainedWords = (primary.trainedWords + other.trainedWords).distinct(),
            baseModel = primary.baseModel ?: other.baseModel,
            sizeBytes = primary.sizeBytes ?: other.sizeBytes,
            downloadUrl = primary.downloadUrl ?: other.downloadUrl,
            pageUrl = primary.pageUrl ?: other.pageUrl,
            sha256 = primary.sha256 ?: other.sha256,
            fileName = primary.fileName ?: other.fileName,
            description = primary.description ?: other.description,
            downloads = primary.downloads ?: other.downloads,
            note = primary.note ?: other.note,
            sources = (a.sources + b.sources).distinct().sortedBy { ModelSource.entries.indexOf(it) },
        )
    }

    private fun score(c: ModelCandidate): Int =
        (if (c.sha256 != null) 4 else 0) +
            (if (c.sizeBytes != null) 2 else 0) +
            (if (c.trainedWords.isNotEmpty()) 1 else 0) +
            (if (c.downloadUrl != null) 1 else 0)

    // -----------------------------------------------------------------------
    //  出网
    // -----------------------------------------------------------------------

    /**
     * 一个源报错时**由它自己说清是什么状态**（而不是让上层去猜代理还是鉴权）。
     */
    private class SourceHttpException(
        val state: Reachability,
        val detail: String,
    ) : RuntimeException(detail)

    /**
     * [CivitaiSearch] 抛的 [ToolFailure] → 可达性。
     *
     * 映射关系直接来自它已经上线的那套错误码（`CIVITAI_TIMEOUT` / `CIVITAI_AUTH` …），
     * 所以"直连超时"与"拒绝了令牌"在一行里就已经分开了，不需要第二条判据。
     */
    private fun failedFromFailure(source: ModelSource, e: ToolFailure): SourceOutcome.Failed {
        val state = when (e.code) {
            // 连接超时 / 连不上：本机多半直连不通（civitai 就是这么个情况，见 OutboundProxy）
            "CIVITAI_TIMEOUT", "CIVITAI_FAILED" -> Reachability.NEEDS_PROXY
            "CIVITAI_AUTH" -> Reachability.NEEDS_LOGIN
            else -> Reachability.ERROR
        }
        return SourceOutcome.Failed(source, state, e.message ?: e.code)
    }

    /**
     * 出一趟网拿一个**顶层是对象**的 JSON（魔搭那两条是；HF 那两条不是，见 [getJsonArray]）。
     *
     * 四条纪律（与 `CivitaiSearch.get` 同一套）：
     *  · 只打调用方写死的域名（没有任何"用户给 URL"的入口）；
     *  · **绝不回显令牌**（日志里只有状态码，错误正文里一个字符都不带）；
     *  · 错误只映射成稳定的状态 + 一句可读的中文；
     *  · 响应正文读完就丢 —— **这里没有任何"把响应写进文件"的路径**（"绝不代下"）。
     */
    private suspend fun request(
        url: String,
        source: ModelSource,
        token: String? = null,
        body: JsonObject? = null,
        method: String = "GET",
    ): JsonObject {
        val text = requestText(url, source, token, body, method)
        return runCatching { AppJson.parseToJsonElement(text).jsonObject }.getOrElse {
            throw SourceHttpException(Reachability.ERROR, "返回了无法解析的内容（顶层不是对象）")
        }
    }

    /**
     * 出一趟网拿一个**顶层是数组**的 JSON。
     *
     * 为什么单独一个：`GET /api/models?search=…`（HF 检索）与 `/tree/main`（HF 目录）
     * 返回的都是数组。第一版用 `request()` 去取，结果每次都被判成
     * "返回了无法解析的内容"——**看起来像 HF 挂了，其实是本地解析错**。
     * 这正是 `FindModelTest` 那条"三源按顺序查到"抓到的 bug。
     */
    private suspend fun getJsonArray(url: String, source: ModelSource, token: String?): JsonArray {
        val text = requestText(url, source, token, body = null, method = "GET")
        return runCatching { AppJson.parseToJsonElement(text).jsonArray }.getOrElse {
            throw SourceHttpException(Reachability.ERROR, "返回了无法解析的内容（顶层不是数组）")
        }
    }

    /** 真正发请求的那一层：只负责"发出去 + 状态码映射"，**不解释正文的形状**。 */
    private suspend fun requestText(
        url: String,
        source: ModelSource,
        token: String? = null,
        body: JsonObject? = null,
        method: String = "GET",
    ): String {
        val uri = runCatching { URI(url) }.getOrElse {
            throw SourceHttpException(Reachability.ERROR, "搜索地址拼装失败（这是本机的 bug）")
        }
        // 带 body 却用 GET 是本机的 bug，不是上游的：魔搭的检索接口是 **PUT**，
        // 而"带 body 就发 GET"会线上吃到 `404 page not found`（实测：同一路径
        // PUT 200 / GET 404）—— 那个 404 会被误读成"上游没这个接口/URL 写错了"。
        // 所以这里当场拦住，而不是让它变成一个看起来像上游问题的状态。
        if (body != null && method == "GET") {
            throw SourceHttpException(Reachability.ERROR, "本机 bug：带请求体却用了 GET（$uri）")
        }
        val builder = HttpRequest.newBuilder(uri)
            .timeout(Duration.ofSeconds(timeoutSeconds.coerceIn(1, FindModel.MAX_TIMEOUT_SECONDS)))
            .header("Accept", "application/json")
        if (token != null) builder.header("Authorization", "Bearer $token")
        if (body != null) {
            builder.header("Content-Type", "application/json")
            val text = AppJson.encodeToString(JsonObject.serializer(), body)
            builder.method(method, HttpRequest.BodyPublishers.ofString(text))
        } else {
            builder.method(method, HttpRequest.BodyPublishers.noBody())
        }

        val response = withContext(Dispatchers.IO) {
            runCatching { client.send(builder.build(), HttpResponse.BodyHandlers.ofString()) }
        }.getOrElse { e ->
            val host = runCatching { URI(url).host }.getOrNull() ?: source.wire
            // 路由写进错误里（"直连"还是"走代理 127.0.0.1:7890"）：用户看不到日志，
            // 但这一行就能让他判断是"本机出不去"还是"对方的问题"。
            //
            // 三种连接失败都带**同一句动作指引**（代理怎么设、别反复重试）：
            // 实测本机 `civitai.com` / `huggingface.co` 直连是 **ConnectException（立刻被拒）**，
            // 不是 `HttpConnectTimeoutException` —— 只在那一种里给指引的话，
            // 最常见的那个现场（本机没有出口）反而拿不到"该怎么办"。
            val route = OutboundProxy.describe(host)
            val guide = "本机直连 $host 不通时，把出网代理设成 127.0.0.1:7890" +
                "（或设环境变量 ${OutboundProxy.ENV_PROXY}）——**不要反复重试**。"
            throw when (e) {
                is HttpConnectTimeoutException -> SourceHttpException(
                    Reachability.NEEDS_PROXY,
                    "连不上 $host：${FindModel.CONNECT_TIMEOUT_SECONDS} 秒内没连上" +
                        "（本次出网路径「$route」）。$guide",
                )
                is HttpTimeoutException -> SourceHttpException(
                    Reachability.NEEDS_PROXY,
                    "连上了 $host 但 $timeoutSeconds 秒没回完（本次出网路径「$route」）。$guide",
                )
                else -> SourceHttpException(
                    Reachability.NEEDS_PROXY,
                    "连不上 $host（${e::class.simpleName}；本次出网路径「$route」）。$guide",
                )
            }
        }

        val status = response.statusCode()
        val text = response.body().orEmpty()
        if (status !in 200..299) {
            // 401 = 没登录 / 令牌无效；403 = gated（HF 要先在网页上同意条款）；
            // 429 = 限流；其余照原样报。日志里**只有状态码**。
            val (state, hint) = when (status) {
                401 -> Reachability.NEEDS_LOGIN to
                    "需要登录 / 令牌（$status）：让用户去「设置 → AI 工具 / 凭据」填一个，别反复试"
                403 -> Reachability.GATED to
                    "被拒绝（$status）：HF 这类 gated 仓库要用户**先在模型页面上同意条款**" +
                    "——这一步只能他自己去点，工具替代不了"
                404 -> Reachability.ERROR to "这一问的目标不存在（$status）"
                429 -> Reachability.ERROR to "被限流了（$status）：等一下再试，别连着搜"
                in 500..599 -> Reachability.ERROR to "对方暂时不可用（$status）"
                else -> Reachability.ERROR to "返回 $status"
            }
            log.warn("{} 查询失败: status={} state={}", source.wire, status, state.wire)
            throw SourceHttpException(state, hint)
        }
        return text
    }

    private fun enc(s: String): String = URLEncoder.encode(s, StandardCharsets.UTF_8)

    /** 是不是一个"用户要手下的"权重文件（按后缀认，与 [FindModel.MODEL_EXTS] 同源）。 */
    private fun looksLikeWeight(path: String): Boolean =
        path.substringAfterLast('.', "").lowercase() in FindModel.MODEL_EXTS
}

/** 严格的 sha256 形状判定（64 位十六进制）—— 上游结构变了也不会把别的东西当成 hash 报出去。 */
internal fun isSha256(raw: String?): Boolean =
    raw != null && raw.length == 64 && raw.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }

/** `size` / `model_size` 这类字段：线上有的给整数、有的给浮点，两种都认；认不出就是 null。 */
internal fun JsonPrimitive.asLong(): Long? =
    runCatching { content.toLong() }.getOrNull()
        ?: runCatching { content.toDouble().toLong() }.getOrNull()
