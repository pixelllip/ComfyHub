package com.comfyhub.ai

import com.comfyhub.AppJson
import kotlinx.serialization.Serializable

/**
 * 内置模型目录的**冻结副本**（用户报的 bug）。
 *
 * 背景：本机的 `%USERPROFILE%\.dsh\settings.yaml` 里登记了 69 个模型，但那是 **DSH 的配置文件**，
 * 装了本项目、没装 DSH 的机器上根本不存在。所以：
 *
 *  - 那份 YAML 的信息已经**抄**成 `server/src/main/resources/ai/builtin-catalog.json`（进 jar、进发布包）；
 *  - 运行期**只读这份资源**，绝不碰文件系统上的 YAML（本类里没有任何 `java.io.File` / YAML 代码）；
 *  - YAML 变了就重跑 `scripts/gen-builtin-catalog.ps1` 再提交（生成结果与资源逐字节一致）。
 *
 * 目录本身只是"出厂默认值"：真正落库由 [AiSeeder] 在启动时幂等完成，落库之后
 * 模型能力的唯一真源就是数据库里的 `ai_models`（AIH-011），用户改了以用户的为准。
 */
object AiSeedCatalog {

    /** classpath 资源路径（前导 `/` 表示从根开始找，`getResourceAsStream` 必须这么写）。 */
    const val RESOURCE = "/ai/builtin-catalog.json"

    @Volatile
    private var cache: BuiltinCatalog? = null

    /**
     * 目录版本，取自资源里的 `version` 字段。
     *
     * 故意**不**硬编码第二份版本号：两份版本号迟早会对不上，而这一份是 [AiSeeder]
     * 写进 `app_settings` 的那个值，必须与资源同源。
     */
    val CATALOG_VERSION: String get() = load().version

    /** 全部内置 provider（当前 1 个）。 */
    fun providers(): List<BuiltinProvider> = load().providers

    /** 按 provider id 找；找不到返回 null（不猜、不回落）。 */
    fun find(providerId: String): BuiltinProvider? =
        load().providers.firstOrNull { it.id == providerId }

    /** settings.yaml 里的 `agent-default-model`（新建会话的默认模型）。 */
    fun agentDefaultModel(): BuiltinAgentDefault? = load().agentDefaultModel

    /** 读 + 解析资源；只在第一次调用时真正解析，之后走缓存。缺资源/坏 JSON 会抛异常。 */
    fun load(): BuiltinCatalog {
        cache?.let { return it }
        synchronized(this) {
            cache?.let { return it }
            val text = javaClass.getResourceAsStream(RESOURCE)
                ?.use { it.readBytes().toString(Charsets.UTF_8) }
                ?: error(
                    "内置模型目录缺失：classpath:$RESOURCE。" +
                        "该文件随 server/src/main/resources/ai/builtin-catalog.json 打包，" +
                        "可用 scripts/gen-builtin-catalog.ps1 重新生成。"
                )
            val parsed = AppJson.decodeFromString(BuiltinCatalog.serializer(), text)
            require(parsed.providers.isNotEmpty()) { "内置模型目录 $RESOURCE 里没有任何 provider" }
            require(parsed.providers.all { it.models.isNotEmpty() }) {
                "内置模型目录 $RESOURCE 里有 provider 没有任何模型"
            }
            cache = parsed
            return parsed
        }
    }
}

/** 冻结副本的顶层结构；与 `builtin-catalog.json` 一一对应。 */
@Serializable
data class BuiltinCatalog(
    val version: String,
    /** 这份副本的来源（人看的，运行期不会去读它） */
    val source: String? = null,
    val note: String? = null,
    val agentDefaultModel: BuiltinAgentDefault? = null,
    val providers: List<BuiltinProvider> = emptyList(),
)

/** `agent-default-model`：provider + model。 */
@Serializable
data class BuiltinAgentDefault(
    val provider: String,
    val model: String,
)

/**
 * 内置 provider。`credentialRef` 只是**环境变量名**，真正的密钥由 [CredentialService] 保管（AIH-012）。
 */
@Serializable
data class BuiltinProvider(
    val id: String,
    val displayName: String,
    val api: String,
    val baseURL: String,
    val credentialRef: String? = null,
    val endpointTrust: String = "public",
    val models: List<BuiltinModel> = emptyList(),
)

/**
 * 内置模型条目。
 *
 * 只声明"模型客观能力"（模态 / 工具 / 推理 / 思考档位 / 上下文窗口）；
 * **不声明** `attachmentTransports` / `mimeAllowlist` / 附件大小上限 ——
 * 协议适配器还没实现附件传输，写了只会是假的（AIH-028/AIH-030）。
 */
@Serializable
data class BuiltinModel(
    val id: String,
    val displayName: String,
    val contextWindow: Int? = null,
    /** 只有 `input: [text, image]` 这一种多模态声明；没写就是仅文本。 */
    val inputModalities: List<String> = listOf("text"),
    val tools: Boolean = true,
    val parallelTools: Boolean = false,
    val reasoning: Boolean = false,
    /** 网关思考方言；本 provider 是 `openai-completions`，就是默认的 `openai`。 */
    val thinkingFormat: String? = null,
    /**
     * 思考档位：等级 → 线上表达。
     *
     * **不含 `off`**：`off` 由 `ReasoningEffort.OFF` 隐含表达，存进 `Map<String,String>` 只会变成噪音。
     * 与 [reasoning] 必须一致：非空 ⇒ `reasoning == true`（保存时 `validateThinkingEfforts` 会拦）。
     */
    val thinkingEfforts: Map<String, String> = emptyMap(),
)
