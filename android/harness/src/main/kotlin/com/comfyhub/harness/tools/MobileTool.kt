package com.comfyhub.harness.tools

import com.comfyhub.harness.kit.RunKit
import com.comfyhub.harness.net.HttpTransport
import com.comfyhub.harness.store.HarnessStore
import kotlinx.coroutines.CompletableDeferred
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import java.io.File
import java.util.concurrent.ConcurrentHashMap

/**
 * 一次"取密钥"的结果。
 *
 * ⚠ **为什么不是 `String?`**（2026-09-27 补的一处诚实缺口）：`null` 同时表示
 * "本机没配这把 key"与"配了但**解不开**"（换过系统用户 / Keystore 里那把钥匙没了）。
 * 把后者当成前者，按两端各自的语义都是**静默降级**：
 *  - 出网工具：用户以为自己在用登录态，实际走的是免密钥 / 匿名通道（AIH-015 的原话：
 *    "把解不开当成没有"是最坏的一种安静）；
 *  - 上游调用：本该清楚的"密钥坏了"会变成一次 401，用户看到的是"模型不认我"。
 *
 * 所以取密钥这件事**必须**能把三件事分开：[Found] / [Missing] / [Unreadable]。
 */
sealed interface SecretLookup {
    /** 取到了明文（**只当场用，不许落库、不许进日志或事件**）。 */
    data class Found(val value: String) : SecretLookup

    /** 本机没配这把 key —— 这是**正常状态**（免密钥 / 匿名通道照常可用）。 */
    object Missing : SecretLookup

    /** 配了，但解不开（密钥库换了、密文坏了）—— **不许当成"没配"**。 */
    object Unreadable : SecretLookup
}

/** 取到明文就用它，其余（没配 / 解不开）都回 `null`。 */
fun SecretLookup.valueOrNull(): String? = (this as? SecretLookup.Found)?.value

/**
 * 工具调用的一次请求（模型给的原始 JSON 文本，**不替它猜类型**）。
 */
data class ToolCallRequest(val callId: String, val name: String, val argumentsJson: String)

/**
 * 工具执行结果。
 *
 * [content] 是**回给模型**的正文（会被截断），[jsonPayload] 是给界面的事件载荷
 * （例如产物入口卡只认 `mediaIds`）。
 */
data class ToolOutcome(
    val status: String,
    val content: String,
    val jsonPayload: JsonObject? = null,
    val errorCode: String? = null,
    val error: String? = null,
) {
    val ok: Boolean get() = status == "success"

    companion object {
        fun success(content: String, jsonPayload: JsonObject? = null) =
            ToolOutcome("success", content, jsonPayload)

        /**
         * 失败**如实报**（带上稳定错误码，让模型能区分"重试有用"与"永远不行"）。
         *
         * 措辞与桌面 `ToolRegistry.fail` 一致：`[工具失败:CODE] message`。
         */
        fun failure(code: String, message: String) =
            ToolOutcome("failed", "[工具失败:$code] $message", errorCode = code, error = message)
    }
}

/**
 * 手机侧的一件事：Skill 目录 + 长期记忆。
 *
 * 手机上这两样都是**本地文件**（`<files>/ai/skills` / `<files>/ai/memory.md`），
 * 与桌面是同一个约定、各自一份（`docs/mobile-harness-design.md` §3）。
 * 走接口注入是为了单测能用临时目录（也便于将来抽 commonMain）。
 */
interface HarnessFiles {
    /** skill 目录（不存在时返回空目录语义，不报错）。 */
    val skillsDir: File

    /** 长期记忆文件。 */
    val memoryFile: File
}

/** 默认布局：`<root>/ai/skills` 与 `<root>/ai/memory.md`（root 通常是 Android 的 `filesDir`）。 */
class DefaultHarnessFiles(private val root: File) : HarnessFiles {
    override val skillsDir: File get() = File(root, "ai/skills")
    override val memoryFile: File get() = File(root, "ai/memory.md")
}

/**
 * `ask_user_question` 的闸门（手机侧）。
 *
 * 与桌面的顺序硬约束一致（红线 **R8**）：**先 [open] 再发 `user.question` 事件** ——
 * 反过来的话用户手快先答，答案会落空，只能等超时。
 */
class MobileAskGate(private val timeoutMs: Long = 10 * 60 * 1000L) {

    private val pending = ConcurrentHashMap<String, CompletableDeferred<AskAnswer>>()

    data class AskAnswer(val text: String, val optionIndex: Int? = null)

    /** 登记一个等回答的提问（**必须在发事件之前**调用）。 */
    fun open(questionId: String) {
        pending[questionId] = CompletableDeferred()
    }

    /** 用户答了。 */
    fun answer(questionId: String, text: String, optionIndex: Int? = null): Boolean =
        pending[questionId]?.complete(AskAnswer(text, optionIndex)) ?: false

    /** 超时 / Run 取消都算"没答"，一律按 [AskAnswer] 的空答案收尾（桌面同语义）。 */
    fun cancel(questionId: String) {
        pending.remove(questionId)?.complete(AskAnswer(""))
    }

    /**
     * 等回答。[timeoutMs] 之后返回 null（调用方按"用户没答"处理，**不许假装答了**）。
     */
    suspend fun await(questionId: String, timeoutMs: Long = this.timeoutMs): AskAnswer? {
        val deferred = pending[questionId] ?: return null
        return kotlinx.coroutines.withTimeoutOrNull(timeoutMs) { deferred.await() }
            .also { pending.remove(questionId) }
    }
}

/**
 * 工具执行上下文 —— 工具 handler **只能从这里拿东西**（策略 / 存储 / 文件 / 闸门）。
 *
 * 与桌面同一条纪律（AGENTS §10）：handler 不许读环境变量或全局单例，
 * 否则"策略在 Run 开始时快照一次"这个前提就破了（用户中途改设置不该影响在飞的请求）。
 */
class ToolContext(
    val runId: String,
    val conversationId: String,
    /** 本次 Run 用的规则工件（工具清单、横幅模板、内联预算都来自它）。 */
    val kit: RunKit,
    val store: HarnessStore,
    val transport: HttpTransport,
    val files: HarnessFiles,
    val askGate: MobileAskGate,
    /** 事件出口：`emit(type, payload)` —— 与桌面同一套 RunEvent 协议。 */
    val emit: (String, JsonObject) -> Unit,
    /**
     * `credential_ref` → 密钥（Android Keystore 那条路由平台通道注入，见
     * `HarnessBridge` 的 `secretLookup`）。
     *
     * **出网工具的 key 只有这一个口子**：`web_search` 取 `keenable`、`civitai_search` 取
     * `civitai`（ref 名与桌面 `credentials::KEENABLE_REF` / `civitai::CREDENTIAL_REF` 同名）。
     * 工具 handler 不许自己去读环境变量或全局单例（与桌面同纪律：策略/凭据在 Run 开始时
     * 快照一次，用户中途改设置不该影响在飞的请求）。
     *
     * ⚠ 返回 [SecretLookup] 而**不是** `String?`：**"没配"与"解不开"是两件事**
     * （[SecretLookup] 的注释写了为什么）。调用方对这两件事的处置也必须不同：
     * 没配是正常状态（免密钥 / 匿名通道照搜），解不开要**如实报码**让人去重填。
     */
    val lookupSecret: (String?) -> SecretLookup = { SecretLookup.Missing },
) {
    /** 一次 Run 里 AI 最多新增几条记忆（与桌面 `MemoryStore.MAX_ENTRIES_PER_RUN` 同值）。 */
    var memoryWrites: Int = 0
    var memoryWritesLimit: Int = 4

    /**
     * 一次 Run 里已经**成功**联网查询几次（`web_fetch` / `web_search` / `civitai_search`
     * **共用这一条**，见 [WebQueryBudget]）。
     *
     * 与桌面执行器同一条纪律：**预检在调用之前、计数只在成功之后**（失败 / 参数错不占额度）。
     */
    var webQueries: Int = 0
    var webQueriesLimit: Int = WebQueryBudget.MAX_QUERIES_PER_RUN

    /**
     * 联网预算的预检（三支出网工具**共用这一份**；各写一份的下场是"某一条工具的额度
     * 悄悄不算数"，而系统提示里那句"一次回复最多联网 8 次"就变成了空话）。
     */
    fun webBudgetFailure(): ToolOutcome? =
        if (webQueries < webQueriesLimit) null
        else ToolOutcome.failure("QUERY_BUDGET_EXCEEDED", WebQueryBudget.exceededMessage(webQueriesLimit))
}

/**
 * 一个手机侧可执行的工具。
 *
 * ⚠ **手机侧实现哪些工具，与"桌面说手机能做哪些"是两件事**：
 * run-kit 的 `tools[]` 说手机能做 7 个（ToolScope 契约 §2），但手机这版只实现了一部分。
 * 没实现的一律**不对外宣称**（见 [MobileToolRegistry.declaredTo]）——
 * 宁可让模型少看见一个工具，也不让它看见一个"调了必然失败"的工具。
 */
interface MobileTool {
    val name: String
    suspend fun invoke(args: JsonObject, ctx: ToolContext): ToolOutcome
}

/**
 * 手机侧的工具表 + 分发。
 *
 * 三条纪律：
 *  1. **只声明自己真实现了的**（[declaredTo]）—— 从 kit 的工具清单里筛选，而不是照单全收；
 *  2. **未知工具明确失败**（`UNKNOWN_TOOL`），不许静默成功；
 *  3. **参数解析失败明确失败**（`BAD_ARGUMENTS`），不许拿空参数硬跑。
 */
class MobileToolRegistry(val tools: List<MobileTool>) {

    private val byName = tools.associateBy { it.name }

    fun find(name: String): MobileTool? = byName[name]

    /**
     * 真正要下发给模型的工具清单：**kit 说的 ∩ 本地实现的**。
     *
     * 为什么不是直接用 kit 的清单：kit 是"桌面认为手机有能力做"的集合，
     * 而这一版手机 harness 还没把每个都实现完。下发给模型一个跑不了的工具，
     * 代价是模型一本正经地调用、然后拿到失败 —— 不如不让它看见。
     */
    fun declaredTo(kit: RunKit): List<com.comfyhub.harness.kit.KitTool> =
        kit.tools.filter { byName.containsKey(it.name) }

    suspend fun invoke(request: ToolCallRequest, ctx: ToolContext): ToolOutcome {
        val tool = byName[request.name]
            ?: return ToolOutcome.failure("UNKNOWN_TOOL", "手机端没有名为 ${request.name} 的工具")
        val args = runCatching {
            com.comfyhub.harness.protocol.ProtocolJson.parseToJsonElement(request.argumentsJson) as? JsonObject
        }.getOrNull() ?: JsonObject(emptyMap())

        return runCatching { tool.invoke(args, ctx) }
            .getOrElse { e ->
                // 工具崩溃不许把整个 Run 带走：如实回报，让模型自己决定下一步
                ToolOutcome.failure("TOOL_INTERNAL_ERROR", "${e::class.simpleName}: ${e.message}")
            }
    }
}

// ---------------------------------------------------------------------------
//  参数取值小工具（模型给的 JSON **不保证类型正确**，一律容错）
// ---------------------------------------------------------------------------

internal fun JsonObject.str(field: String): String? =
    (this[field] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }

internal fun JsonObject.int(field: String): Int? =
    (this[field] as? JsonPrimitive)?.contentOrNull?.toIntOrNull()

internal fun JsonObject.bool(field: String): Boolean? =
    (this[field] as? JsonPrimitive)?.contentOrNull?.toBooleanStrictOrNull()

internal fun JsonObject.strList(field: String): List<String> =
    (this[field] as? kotlinx.serialization.json.JsonArray)
        ?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull?.takeIf { s -> s.isNotBlank() } }
        .orEmpty()
