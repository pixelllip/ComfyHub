package com.comfyhub.harness.bridge

import android.content.Context
import com.comfyhub.harness.agent.HarnessRunner
import com.comfyhub.harness.agent.RunRequest
import com.comfyhub.harness.event.RunEventBus
import com.comfyhub.harness.kit.Decision
import com.comfyhub.harness.kit.HarnessJson
import com.comfyhub.harness.kit.KitStatus
import com.comfyhub.harness.kit.RunKit
import com.comfyhub.harness.kit.RunKitCache
import com.comfyhub.harness.kit.RunKitClient
import com.comfyhub.harness.kit.RunKitCodec
import com.comfyhub.harness.kit.RunKitGuard
import com.comfyhub.harness.net.UrlConnectionTransport
import com.comfyhub.harness.protocol.AiApiRef
import com.comfyhub.harness.protocol.ReasoningEffort
import com.comfyhub.harness.store.HarnessDatabase
import com.comfyhub.harness.store.HarnessStore
import com.comfyhub.harness.store.MessageStatus
import com.comfyhub.harness.tools.BuiltinTools
import com.comfyhub.harness.tools.DefaultHarnessFiles
import com.comfyhub.harness.tools.MobileAskGate
import com.comfyhub.harness.tools.MobileToolRegistry
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.File

/**
 * Flutter ↔ Kotlin harness 的桥（`MethodChannel` 调用 + `EventChannel` 事件流）。
 *
 * ## 为什么事件流必须走 `EventChannel`
 *
 * 手机上的 Run 事件由**本地 harness** 产生，而渲染它的 UI 与桌面**是同一套代码**
 * （`docs/mobile-harness-design.md` §5 的价值全在这里）。所以事件流也要按同一份
 * `RunEvent` 协议往外送 —— 只是传输从"远端 SSE"换成"本地通道"：
 *
 * | | 桌面 | 手机 |
 * | --- | --- | --- |
 * | 生产者 | 后端 `RunEventBus` | 本地 `RunEventBus`（同一个类语义） |
 * | 传输 | SSE（`id/event/data`） | `EventChannel`（`{seq, type, payloadJson}`） |
 * | 消费者 | Flutter `RunEventSource` 的远端实现 | 同一个 `RunEventSource` 的本地实现 |
 *
 * ## 大文件不走通道
 *
 * 附件 / 产物**只传路径或 URL**，绝不把字节塞进通道（会来回拷贝）。
 *
 * ## 一次 Run 的生命周期
 *
 * ```
 * Dart: startRun(...)      → 建会话/消息/Run，返回 runId（事件总线**同时**已 open）
 * Dart: events(runId)      → onListen({runId, after}) → 泵事件到 EventSink
 * Dart: cancelRun(runId)   → 取消协程（R6 兜底会把已说的话保住）
 * 收尾                     → 总线 close → 通道那侧的 Stream 自然结束
 * ```
 */
class HarnessBridge(private val context: Context) :
    MethodChannel.MethodCallHandler, EventChannel.StreamHandler {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val transport = UrlConnectionTransport()
    private val credentials = KeystoreCredentials(context)
    private val bus = RunEventBus()

    private var database: HarnessDatabase? = null
    private var store: HarnessStore? = null
    private var runner: HarnessRunner? = null
    private var methodChannel: MethodChannel? = null
    private var eventChannel: EventChannel? = null
    private var pumpJob: Job? = null
    private var sink: EventChannel.EventSink? = null

    /** 绑到 Activity 的引擎上（`MainActivity.configureFlutterEngine` 调）。 */
    fun attach(messenger: BinaryMessenger) {
        methodChannel = MethodChannel(messenger, METHOD_CHANNEL).also { it.setMethodCallHandler(this) }
        eventChannel = EventChannel(messenger, EVENT_CHANNEL).also { it.setStreamHandler(this) }
        // 建库放在 attach 里（不是第一次调用时）：自检不过要**当场**炸，
        // 而不是等用户发出第一条消息才发现本地库是坏的
        ensureInitialized()
    }

    fun detach() {
        pumpJob?.cancel()
        methodChannel?.setMethodCallHandler(null)
        eventChannel?.setStreamHandler(null)
        methodChannel = null
        eventChannel = null
        runner?.shutdown()
    }

    // -----------------------------------------------------------------------
    //  初始化
    // -----------------------------------------------------------------------

    private fun ensureInitialized(): HarnessStore {
        store?.let { return it }
        val db = HarnessDatabase.open(File(context.filesDir, HarnessDatabase.DB_FILE).absolutePath)
        val s = HarnessStore(db.session)
        val r = HarnessRunner(
            store = s,
            bus = bus,
            tools = MobileToolRegistry(BuiltinTools.all()),
            transport = transport,
            files = DefaultHarnessFiles(context.filesDir),
            askGate = MobileAskGate(),
            resolveSecret = { ref -> ref?.let { credentials.get(it) } },
            scope = scope,
        )
        database = db
        store = s
        runner = r
        return s
    }

    // -----------------------------------------------------------------------
    //  MethodChannel
    // -----------------------------------------------------------------------

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        try {
            when (call.method) {
                "initialize" -> result.success(capabilities())
                "capabilities" -> result.success(capabilities())
                "fetchRunKit" -> fetchRunKit(call, result)
                "cachedRunKit" -> result.success(cachedKitJson())
                "startRun" -> startRun(call, result)
                "cancelRun" -> result.success(runner?.cancel(call.argument<String>("runId").orEmpty()) ?: false)
                "answerQuestion" -> result.success(
                    runner?.answerQuestion(
                        questionId = call.argument<String>("questionId").orEmpty(),
                        text = call.argument<String>("answer").orEmpty(),
                        optionIndex = call.argument<Int>("optionIndex"),
                    ) ?: false,
                )
                "listConversations" -> result.success(listConversations())
                "listMessages" -> result.success(listMessages(call.argument<String>("conversationId").orEmpty()))
                "listParts" -> result.success(listParts(call.argument<String>("messageId").orEmpty()))
                "conversationTitle" -> result.success(
                    store?.getConversation(call.argument<String>("conversationId").orEmpty())?.title,
                )
                "pendingOutbox" -> result.success(store?.pendingOutbox()?.size ?: 0)
                "toolNames" -> result.success(BuiltinTools.implementedNames().sorted())
                "installRunKit" -> result.success(installRunKit(call.argument<String>("kitJson").orEmpty()))
                "credentialRefs" -> result.success(credentials.refs())
                "hasCredential" -> result.success(
                    credentials.has(call.argument<String>("ref").orEmpty()),
                )
                "putCredential" -> {
                    val ref = call.argument<String>("ref").orEmpty()
                    val value = call.argument<String>("value").orEmpty()
                    if (ref.isEmpty() || value.isEmpty()) {
                        result.error("BAD_ARGUMENTS", "ref 与 value 都不能为空", null)
                    } else {
                        credentials.put(ref, value)
                        result.success(true)
                    }
                }
                "removeCredential" -> {
                    credentials.remove(call.argument<String>("ref").orEmpty())
                    result.success(true)
                }
                "recoverUnfinished" -> result.success(recoverUnfinished())
                else -> result.notImplemented()
            }
        } catch (e: Throwable) {
            result.error("HARNESS_ERROR", "${e::class.simpleName}: ${e.message}", null)
        }
    }

    private fun capabilities(): Map<String, Any?> = mapOf(
        // schemaVersion 与桌面的 `PRAGMA user_version` / `/api/sync/manifest` 是同一个数
        "schemaVersion" to com.comfyhub.harness.store.MobileSchema.VERSION,
        "dbPath" to File(context.filesDir, HarnessDatabase.DB_FILE).absolutePath,
        "toolNames" to BuiltinTools.implementedNames().sorted(),
        "cachedKitVersion" to store?.let { RunKitCache(it).cachedVersion() },
        "credentialRefs" to credentials.refs(),
    )

    /**
     * 取 run-kit（规则工件）并缓存。
     *
     * 拿不到就用缓存（B 方案的定义就是"桌面不在线也能继续出方案"）——
     * 判定逻辑在 [RunKitGuard]，**没规则就不许跑**（宁可不跑，也不给模型一套瞎编的规则）。
     */
    private fun fetchRunKit(call: MethodCall, result: MethodChannel.Result) {
        val s = ensureInitialized()
        val baseUrl = call.argument<String>("baseUrl").orEmpty()
        val token = call.argument<String>("token")
        val providerId = call.argument<String>("providerId")
        val modelId = call.argument<String>("modelId")
        val cache = RunKitCache(s)

        var fetched: RunKit? = null
        var status: KitStatus? = null
        var fetchError: String? = null
        if (baseUrl.isNotEmpty()) {
            val client = RunKitClient(transport, baseUrl, token)
            runCatching { client.status() }.onSuccess { status = it }.onFailure { fetchError = it.message }
            runCatching { client.fetch(providerId, modelId) }
                .onSuccess { kit ->
                    fetched = kit
                    cache.save(kit)
                }
                .onFailure { fetchError = it.message }
        }

        when (val decision = RunKitGuard.decide(fetched ?: cache.load(), status)) {
            is Decision.Run -> result.success(
                buildJsonObject {
                    put("ok", true)
                    put("fresh", fetched != null)
                    put("kitVersion", decision.kit.kitVersion)
                    decision.warning?.let { put("warning", it) }
                    put(
                        "kit",
                        com.comfyhub.harness.kit.HarnessJson.parse(
                            kitJsonOrEmpty(fetched ?: decision.kit)
                        ),
                    )
                }.toString(),
            )
            is Decision.Refresh -> result.success(
                buildJsonObject {
                    put("ok", false)
                    put("reason", decision.reason)
                    fetchError?.let { put("error", it) }
                }.toString(),
            )
            Decision.Blocked -> result.success(
                buildJsonObject {
                    put("ok", false)
                    put("reason", "本地还没有规则工件（run-kit），且当前连不上桌面")
                    fetchError?.let { put("error", it) }
                }.toString(),
            )
        }
    }

    private fun kitJsonOrEmpty(kit: RunKit): String = com.comfyhub.harness.kit.RunKitCodec.toJson(kit)

    private fun cachedKitJson(): String? = store?.let { RunKitCache(it).load()?.let(::kitJsonOrEmpty) }

    /** 桌面离线时也允许把一份 kit 直接塞进来（同步通道 / 调试用）。 */
    private fun installRunKit(kitJson: String): Boolean {
        if (kitJson.isBlank()) return false
        val kit = runCatching { RunKitCodec.parse(kitJson) }.getOrNull() ?: return false
        store?.let { RunKitCache(it).save(kit) }
        return true
    }

    private fun startRun(call: MethodCall, result: MethodChannel.Result): Unit {
        val s = ensureInitialized()
        val kitJson = call.argument<String>("kitJson")
        val kit = when {
            !kitJson.isNullOrBlank() -> RunKitCodec.parse(kitJson)
            else -> RunKitCache(s).load() ?: run {
                result.error("NO_RUN_KIT", "还没有规则工件（run-kit），先联网取一次", null)
                return
            }
        }
        val api = AiApiRef.parse(call.argument<String>("api"))
        if (api == null) {
            result.error("BAD_ARGUMENTS", "未知协议：${call.argument<String>("api")}", null)
            return
        }
        val request = RunRequest(
            conversationId = call.argument<String>("conversationId") ?: com.comfyhub.harness.store.Timestamps.newId(),
            providerId = call.argument<String>("providerId").orEmpty(),
            modelId = call.argument<String>("modelId").orEmpty(),
            api = api,
            baseUrl = call.argument<String>("baseUrl").orEmpty(),
            credentialRef = call.argument<String>("credentialRef"),
            kit = kit,
            userText = call.argument<String>("text").orEmpty(),
            reasoningEffort = ReasoningEffort.parse(call.argument<String>("reasoningEffort")),
            maxSteps = call.argument<Int>("maxSteps") ?: 8,
        )
        val runId = runner?.start(request)
            ?: run {
                result.error("NOT_INITIALIZED", "harness 还没初始化", null)
                return
            }
        result.success(
            buildJsonObject {
                put("runId", runId)
                put("conversationId", request.conversationId)
                put("kitVersion", kit.kitVersion)
                put("promptVersion", kit.promptVersion)
            }.toString(),
        )
    }

    // -----------------------------------------------------------------------
    //  列表查询（手机自己在本地出方案，界面要能读到本地会话）
    // -----------------------------------------------------------------------

    private fun listConversations(): String {
        val s = ensureInitialized()
        return buildJsonArray {
            s.listConversations().forEach { c ->
                add(buildJsonObject {
                    put("id", c.id)
                    put("title", c.title)
                    put("providerId", c.providerId)
                    put("modelId", c.modelId)
                    put("updatedAt", c.updatedAt)
                })
            }
        }.toString()
    }

    private fun listMessages(conversationId: String): String {
        val s = ensureInitialized()
        return buildJsonArray {
            s.listMessages(conversationId).forEach { m ->
                add(buildJsonObject {
                    put("id", m.id)
                    put("conversationId", m.conversationId)
                    put("seq", m.seq)
                    put("role", m.role)
                    put("status", m.status)
                    put("text", m.text)
                    put("createdAt", m.createdAt)
                })
            }
        }.toString()
    }

    private fun listParts(messageId: String): String {
        val s = ensureInitialized()
        return buildJsonArray {
            s.listParts(messageId).forEach { p ->
                add(buildJsonObject {
                    put("type", p.type)
                    put("text", p.text)
                    put("toolCallId", p.toolCallId)
                    put("jsonPayload", p.jsonPayload)
                })
            }
        }.toString()
    }

    /**
     * 崩溃恢复：把上次没跑完的 Run 按 **R6** 收尾。
     *
     * 手机上前台服务有 6h/24h 配额、后台随时被杀（总纲 §7 风险表），
     * 所以进程重启后必须能认出"有一个 Run 没跑完"，而不是留一个永远 `running` 的幽灵。
     * 这里只做**诚实收尾**：状态记 failed（含错误说明），已经落库的正文与块一个字不动。
     */
    private fun recoverUnfinished(): Int {
        val s = ensureInitialized()
        val unfinished = s.listUnfinishedRuns()
        unfinished.forEach { run ->
            s.updateMessage(
                run.assistantMessageId.orEmpty(),
                status = MessageStatus.FAILED,
            )
            s.finishRun(
                run.id,
                com.comfyhub.harness.store.RunStatus.FAILED,
                errorCode = "APP_KILLED",
                errorMessage = "上一次回复在应用被杀时中断（已保留已经说出的内容）",
            )
        }
        return unfinished.size
    }

    // -----------------------------------------------------------------------
    //  EventChannel
    // -----------------------------------------------------------------------

    override fun onListen(arguments: Any?, events: EventChannel.EventSink) {
        sink = events
        ensureInitialized()
        val args = arguments as? Map<*, *>
        val runId = args?.get("runId") as? String
        val after = (args?.get("after") as? Number)?.toInt() ?: 0
        if (runId.isNullOrBlank()) {
            // 没有指定 Run：只回一条空心跳，避免 Dart 侧永远等不到第一帧
            events.success(mapOf("seq" to 0, "type" to "heartbeat", "payloadJson" to "{}"))
            return
        }
        startPump(runId, after)
    }

    override fun onCancel(arguments: Any?) {
        pumpJob?.cancel()
        pumpJob = null
        sink = null
    }

    private fun startPump(runId: String, after: Int) {
        pumpJob?.cancel()
        pumpJob = scope.launch {
            val channel = bus.subscribe(runId, after)
            if (channel == null) {
                // 总线里没有（进程重启过 / Run 已收尾并被清理）→ 从**本地库**回放（R5）
                store?.listEvents(runId, after)?.forEach { (seq, type) ->
                    sink?.success(mapOf("seq" to seq, "type" to type, "payloadJson" to "{}"))
                }
                sink?.endOfStream()
                return@launch
            }
            for (event in channel) {
                sink?.success(
                    mapOf(
                        "seq" to event.seq,
                        "type" to event.type,
                        "payloadJson" to event.payload.toString(),
                    )
                )
            }
            sink?.endOfStream()
        }
    }

    companion object {
        /** 方法通道名（Dart 侧 `MethodChannel('comfyhub/harness')`）。 */
        const val METHOD_CHANNEL = "comfyhub/harness"

        /** 事件通道名（Dart 侧 `EventChannel('comfyhub/harness/events')`）。 */
        const val EVENT_CHANNEL = "comfyhub/harness/events"
    }
}

