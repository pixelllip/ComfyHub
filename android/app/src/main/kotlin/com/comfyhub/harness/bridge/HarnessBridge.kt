package com.comfyhub.harness.bridge

import android.content.Context
import com.comfyhub.harness.agent.HarnessRunner
import com.comfyhub.harness.agent.MentionSpec
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
import com.comfyhub.harness.store.ProviderJson
import com.comfyhub.harness.store.ProviderStoreFailure
import com.comfyhub.harness.sync.SyncPullClient
import com.comfyhub.harness.sync.SyncPushClient
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
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonPrimitive
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

    /**
     * 把 APK 里那几份提示词 skill 种进 `<filesDir>/ai/skills`。
     *
     * 为什么手机需要它：skill 正文的真源是**桌面**磁盘，而手机 `load_skill` 读的是本机投放口。
     * 不种的话，系统提示里列着的 skill 一个都加载不到 —— 用户看到的 `/技能` 只有一个名字。
     *
     * 失败只记日志、不拦启动：`list_skills` 会如实说本机没有，比"假装装好了"好。
     */
    private fun seedBuiltinSkills() {
        val target = File(context.filesDir, "ai/skills")
        runCatching {
            val result = com.comfyhub.harness.tools.SkillSeeder.seed(
                AssetsSkillSource(context),
                target,
            )
            if (result.failed > 0) {
                android.util.Log.w(
                    "ComfyHub",
                    "内置 skill 有 ${result.failed} 份没种进去（投放口：${target.absolutePath}）",
                )
            }
        }.onFailure {
            android.util.Log.w("ComfyHub", "内置 skill 种植失败：${it.message}")
        }
    }

    private fun ensureInitialized(): HarnessStore {
        store?.let { return it }
        // 先把随包内置的提示词 skill 种进本机投放口（只种一次、绝不覆盖）。
        // 放在建库之前、且失败**不拦启动**：少几份 skill 只是 AI 少一套规则，
        // 而库起不来就什么都做不了了。
        seedBuiltinSkills()
        val db = HarnessDatabase.open(File(context.filesDir, HarnessDatabase.DB_FILE).absolutePath)
        val s = HarnessStore(db.session)
        val r = HarnessRunner(
            store = s,
            bus = bus,
            tools = MobileToolRegistry(BuiltinTools.all()),
            transport = transport,
            files = DefaultHarnessFiles(context.filesDir),
            askGate = MobileAskGate(),
            // ⚠ 这条 lambda 返回的是 `SecretLookup`（三态），**不是 `String?`**：
            //    "没配"与"解不开"必须分开（AIH-015）—— 前者是正常状态（免密钥 / 匿名通道照搜），
            //    后者要如实报码让人去重填。合成一个 `null` 就是静默降级。
            secretLookup = { ref -> credentials.lookup(ref.orEmpty()) },
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
                // ⚠ 这两个**要出网**，必须离开主线程 ——
                // Android 禁止在主线程做网络 I/O，直接调会抛
                // `NetworkOnMainThreadException`：表现是 run-kit 永远取不到
                // （于是发送按钮一直是灰的）、上行永远推不上去，
                // 而两端都不报错。2026-09-24 真机验收就是这么抓到的。
                "fetchRunKit" -> offMain(result) { fetchRunKit(call) }
                "syncNow" -> offMain(result) { syncNow(call) }
                // 下行（M4 的另一半）：与 syncNow 同一个理由要离开主线程 —— 这是一串 GET
                "syncPull" -> offMain(result) { syncPull(call) }
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
                "createConversation" -> {
                    val s = ensureInitialized()
                    val id = call.argument<String>("conversationId")
                        ?: com.comfyhub.harness.store.Timestamps.newId()
                    val record = s.createConversation(
                        id = id,
                        title = call.argument<String>("title") ?: "新对话",
                        providerId = call.argument<String>("providerId"),
                        modelId = call.argument<String>("modelId"),
                        promptVersion = call.argument<String>("promptVersion") ?: "v1",
                    )
                    result.success(
                        buildJsonObject {
                            put("id", record.id)
                            put("title", record.title)
                            put("providerId", record.providerId)
                            put("modelId", record.modelId)
                            put("updatedAt", record.updatedAt)
                        }.toString(),
                    )
                }
                "listMessages" -> result.success(listMessages(call.argument<String>("conversationId").orEmpty()))
                // 清掉本地镜像里的空壳会话（界面切会话时顺手做的那种清理）。
                // **只动本地库**：远端那条 DELETE 在手机上是打给桌面的，够不着本地镜像 ——
                // 这正是"手机上空壳越攒越多"的原因（见 HarnessStore.deleteConversation）。
                // 回的是"本机是否真的删掉了一行"：false = 本来就没有（不是错误）。
                "deleteConversation" -> result.success(
                    ensureInitialized().deleteConversation(call.argument<String>("conversationId").orEmpty()),
                )
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
                // **把密钥明文取出来**的那一个动作。现在有**两个**调用方，都必须显式：
                //  ① 配对令牌要进请求头（`X-ComfyHub-Token`，`docs/pairing-design.md` §3.2 第 13 步）；
                //  ② 用户点「把本机密钥推给桌面」（`CredentialStore.reveal()`，§14.4）——
                //     那是**全仓唯一**允许读密钥的地方，名字刻意不叫 read/get，评审时一眼能看见。
                // 三条不变：值只回给 Dart 的内存，**不进日志、不进事件、不回显**。
                "getCredential" -> result.success(
                    credentials.get(call.argument<String>("ref").orEmpty()),
                )
                // 配对付给桌面的身份：deviceId 必须与上行同步用的**同一个**
                // （`SyncPushClient.deviceId` 的纪律：生成两次 = 桌面当成两台设备）。
                "pairingIdentity" -> result.success(pairingIdentity())
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

                // ---- Provider / 模型目录（M6 的另一半）---------------------------------
                // 手机**自己那份清单**：ai_providers / ai_models 是**本机表**（不进同步范围），
                // 所以这几个方法**一条网络请求都不打** —— 桌面不在线时也能新建/改地址。
                // 进出都是 **JSON 字符串**，键名与 `AiProviderDto` / `AiModelDto` 逐字对齐
                // （Dart 侧就是 `AiProvider.toJson()` / `AiModel.toJson()` 那份形状）。
                "listProviders" -> result.success(listProviders())
                "saveProvider" -> saveProvider(call, result)
                "deleteProvider" -> deleteProvider(call, result)
                "listModels" -> listModels(call, result)
                "saveModels" -> saveModels(call, result)
                else -> result.notImplemented()
            }
        } catch (e: Throwable) {
            result.error("HARNESS_ERROR", "${e::class.simpleName}: ${e.message}", null)
        }
    }

    /**
     * 把**会出网**的桥方法挪到后台线程，结果再回主线程交给 [result]。
     *
     * `MethodChannel.Result` **只能在主线程回调**（Flutter 的规定），
     * 而 Android 又不许在主线程做网络 I/O —— 两头都要顾，所以是
     * "后台算 → `withContext(Main)` 回"。
     */
    private fun offMain(result: MethodChannel.Result, block: () -> String) {
        scope.launch {
            val outcome = runCatching { block() }
            withContext(Dispatchers.Main) {
                outcome.fold(
                    onSuccess = { result.success(it) },
                    onFailure = {
                        result.error("HARNESS_ERROR", "${it::class.simpleName}: ${it.message}", null)
                    },
                )
            }
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
     * 取 run-kit（规则工件）并缓存。**返回 JSON 字符串**，由 [offMain] 回给 Dart。
     *
     * 拿不到就用缓存（B 方案的定义就是"桌面不在线也能继续出方案"）——
     * 判定逻辑在 [RunKitGuard]，**没规则就不许跑**（宁可不跑，也不给模型一套瞎编的规则）。
     */
    private fun fetchRunKit(call: MethodCall): String {
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

        return when (val decision = RunKitGuard.decide(fetched ?: cache.load(), status)) {
            is Decision.Run -> buildJsonObject {
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
            }.toString()
            is Decision.Refresh -> buildJsonObject {
                put("ok", false)
                put("reason", decision.reason)
                fetchError?.let { put("error", it) }
            }.toString()
            Decision.Blocked -> buildJsonObject {
                put("ok", false)
                put("reason", "本地还没有规则工件（run-kit），且当前连不上桌面")
                fetchError?.let { put("error", it) }
            }.toString()
        }
    }

    private fun kitJsonOrEmpty(kit: RunKit): String = com.comfyhub.harness.kit.RunKitCodec.toJson(kit)

    /**
     * 配对时报给桌面的身份：`{deviceId, deviceName}`（🔒 `docs/pairing-design.md` §11.2）。
     *
     * `deviceId` 走 [SyncPushClient.deviceId]（与上行同步同一个），
     * `deviceName` 用 `Build.MODEL`（桌面上那一行"哪台手机"要给人看）。
     */
    private fun pairingIdentity(): String {
        val s = ensureInitialized()
        val id = SyncPushClient(s, transport).deviceId()
        val name = android.os.Build.MODEL?.takeIf { it.isNotBlank() } ?: "Android 设备"
        return buildJsonObject {
            put("deviceId", id)
            put("deviceName", name)
        }.toString()
    }

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
        // `@` 卡片 / `/技能` 的发送时快照（Dart 侧 `OutgoingMention.toJson()` 的数组）。
        // 解析不了就当场回 BAD_ARGUMENTS —— 静默当空数组会把用户选的卡悄悄丢掉。
        val mentions = try {
            parseMentions(call.argument<String>("mentions"))
        } catch (e: IllegalArgumentException) {
            result.error("BAD_ARGUMENTS", e.message ?: "mentions 读不懂", null)
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
            // 界面画气泡用这两样：`displayText` 是用户看到的正文，`mentions` 是小卡片。
            // 少了它们，重开会话时那条气泡会退化成一大段卡正文（与刚发出时长得不一样）。
            displayText = call.argument<String>("displayText"),
            mentions = mentions,
            reasoningEffort = ReasoningEffort.parse(call.argument<String>("reasoningEffort")),
            maxSteps = call.argument<Int>("maxSteps") ?: 8,
            // 界面自己生成的 id：它已经按这两个 id 插好气泡了，库里必须用同一对
            userMessageId = call.argument<String>("userMessageId"),
            assistantMessageId = call.argument<String>("assistantMessageId"),
        )
        val started = runner?.start(request)
            ?: run {
                result.error("NOT_INITIALIZED", "harness 还没初始化", null)
                return
            }
        result.success(
            buildJsonObject {
                put("runId", started.runId)
                put("conversationId", started.conversationId)
                // 这两个是 M3.2 补的：只回 runId 的话，界面手里那两条乐观气泡找不到主人
                put("userMessageId", started.userMessageId)
                put("assistantMessageId", started.assistantMessageId)
                put("kitVersion", kit.kitVersion)
                put("promptVersion", kit.promptVersion)
            }.toString(),
        )
    }

    /**
     * 解析 Dart 侧送来的 `mentions`（JSON **数组字符串**）。
     *
     * 形状与桌面 `POST /api/ai/conversations/{id}/runs` 的 `mentions[]` 同一套键名：
     * `{kind, name, modelText?, snapshot?}`。这里**只做形状校验**，内容不校验
     * （卡片是用户自己的东西，跟桌面同一条纪律）；解析不了就抛，由调用方回 `BAD_ARGUMENTS`
     * —— 静默当空数组等于把用户选的卡悄悄丢掉，界面上那张小卡片就永远画不出来。
     *
     * 上限与桌面同值（`MAX_MENTIONS = 32`）：超了直接说，不截断。
     */
    private fun parseMentions(raw: String?): List<MentionSpec> {
        if (raw.isNullOrBlank()) return emptyList()
        val element = Json.parseToJsonElement(raw)
        val array = element as? JsonArray
            ?: throw IllegalArgumentException("mentions 必须是一段 JSON 数组")
        if (array.size > MAX_MENTIONS) {
            throw IllegalArgumentException("一条消息最多带 $MAX_MENTIONS 个卡片或技能引用")
        }
        return array.map { item ->
            val obj = item as? JsonObject
                ?: throw IllegalArgumentException("mentions 里每一项都必须是 JSON 对象")
            val kind = obj["kind"]?.jsonPrimitive?.contentOrNull?.trim().orEmpty()
            val name = obj["name"]?.jsonPrimitive?.contentOrNull?.trim().orEmpty()
            if (kind.isEmpty()) throw IllegalArgumentException("mentions 里有一项没有 kind")
            if (name.isEmpty()) throw IllegalArgumentException("mentions 里有一项没有名字")
            // 快照原样留着（它是发送那一刻的只读副本），到落库时才序列化
            val snapshot = obj["snapshot"]?.takeIf { it is JsonObject }?.toString()
            MentionSpec(
                kind = kind,
                name = name,
                modelText = obj["modelText"]?.jsonPrimitive?.contentOrNull,
                snapshotJson = snapshot,
            )
        }
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
    //  上行（M4）：把本机 `_outbox` 推到桌面
    // -----------------------------------------------------------------------

    /**
     * 推一批本机产生的会话 / 消息 / 块到桌面（契约 §4）。
     *
     * 只推**一批**（默认 200 条）：调用方想推完就再调一次 ——
     * 这样"还剩多少"永远是真实读数，中途被打断也不会丢东西。
     *
     * 拿不到后端地址时**如实回 `ok=false`**（不是静默成功）：界面上"已同步"是假的，
     * 用户会以为桌面上已经有了。
     */
    private fun syncNow(call: MethodCall): String {
        val s = ensureInitialized()
        val baseUrl = call.argument<String>("baseUrl").orEmpty().trim()
        val token = call.argument<String>("token")
        if (baseUrl.isEmpty()) {
            return buildJsonObject {
                put("ok", false)
                put("reason", "还不知道桌面地址（本机没有连过桌面）")
            }.toString()
        }
        val outcome = SyncPushClient(s, transport).push(baseUrl.trimEnd('/'), token)
        return buildJsonObject {
            // `ok` 的口径：这一批**没有留下暂时性失败**才算成功
            put("ok", outcome.error == null && outcome.rejectedTransient == 0)
            put("applied", outcome.applied)
            put("rejectedPermanent", outcome.rejectedPermanent)
            put("rejectedTransient", outcome.rejectedTransient)
            put("pending", outcome.pending)
            put("droppedLocal", outcome.droppedLocal)
            outcome.error?.let { put("error", it) }
        }.toString()
    }

    // -----------------------------------------------------------------------
    //  下行（M4 的另一半）：把桌面那份投影到本地镜像
    // -----------------------------------------------------------------------

    /**
     * 拉一次（`GET /api/sync/{manifest,changes,bootstrap}` → 本地镜像）。
     *
     * 形状与上行那条对齐：`{ok, mode, applied, cursor, skippedDeleted?, skippedUnknown?,
     * mirrorDropped?, error?}` —— 多的几个键只用于**如实说明**（跳过了几行、
     * 这次为了重建镜像丢了多少行），Dart 侧不读也不影响。
     *
     * ⚠ `mode` 只在 `ok=true` 时是结论；`ok=false` 时别拿它当结论（判据永远是 `ok`）。
     * 拿不到后端地址时**如实回 `ok=false`**（不是静默成功）—— 界面上说"已同步"是假的，
     * 用户会以为桌面上那份已经下来了。
     */
    private fun syncPull(call: MethodCall): String {
        val s = ensureInitialized()
        val baseUrl = call.argument<String>("baseUrl").orEmpty().trim()
        val token = call.argument<String>("token")
        if (baseUrl.isEmpty()) {
            return buildJsonObject {
                put("ok", false)
                put("mode", SyncPullClient.MODE_UP_TO_DATE)
                put("applied", 0)
                put("cursor", s.syncCursor())
                put("error", "还不知道桌面地址（本机没有连过桌面）")
            }.toString()
        }
        val outcome = SyncPullClient(s, transport).pullOnce(baseUrl.trimEnd('/'), token)
        return buildJsonObject {
            put("ok", outcome.ok)
            put("mode", outcome.mode)
            put("applied", outcome.applied)
            put("cursor", outcome.cursor)
            put("skippedDeleted", outcome.skippedDeleted)
            put("skippedUnknown", outcome.skippedUnknown)
            put("mirrorDropped", outcome.mirrorDropped)
            outcome.error?.let { put("error", it) }
        }.toString()
    }

    // -----------------------------------------------------------------------
    //  Provider / 模型目录（**只动本机库，不出网**）
    // -----------------------------------------------------------------------

    /**
     * `listProviders()` → JSON **数组**字符串。
     *
     * ⚠ **不写 `credential` 键**：那是"这台手机有没有 key"的即时状态，真源是本机密钥库，
     * 界面用 `CredentialStore.statusOf` 现问（`AiProvider.fromJson` 缺这个键时给 `none`，
     * 接着被覆盖）。桥**不编一个可能过期的状态** —— 那正是"显示已配置、点发送却 401"的老坑。
     */
    private fun listProviders(): String = ProviderJson.providersToJsonArray(ensureInitialized().listProviders())

    /**
     * `saveProvider(provider: <JSON 字符串>)` → 保存后的那条（JSON 字符串）。
     *
     * `revision` 缺省/0 = 新建；带正数 = 乐观锁更新 —— 语义与错误文案都照桌面
     * （见 `HarnessStore.saveProvider`）。
     */
    private fun saveProvider(call: MethodCall, result: MethodChannel.Result) {
        val raw = call.argument<String>("provider").orEmpty()
        val json = ProviderJson.parseObject(raw)
        if (json == null) {
            result.error("BAD_ARGUMENTS", "provider 必须是一段 JSON 对象", null)
            return
        }
        val store = ensureInitialized()
        val record = try {
            ProviderJson.providerFromJson(json)
        } catch (e: ProviderJson.ProviderJsonFailure) {
            result.error("BAD_ARGUMENTS", e.message, null)
            return
        }
        try {
            result.success(ProviderJson.providerToJson(store.saveProvider(record)).toString())
        } catch (e: ProviderStoreFailure) {
            // 桌面对应的三条都是 400 CONFIG_ERROR —— 这里用同一个码，Dart 侧按人话显示
            result.error("CONFIG_ERROR", e.message, null)
        }
    }

    /**
     * `deleteProvider(id)` → 是否真的删掉了一行；不存在按桌面同样是**错误**（"Provider 不存在: x"）。
     *
     * **密钥跟着走**（2026-09-28 用户 bug，与桌面 `ai::delete_provider` 同一条纪律）：
     * provider 删掉后，它那把本机密钥若已无别的 provider 引用，一并从本机密钥库清掉 ——
     * 否则它就成了界面上再也摸不到的孤儿。
     */
    private fun deleteProvider(call: MethodCall, result: MethodChannel.Result) {
        val id = call.argument<String>("id").orEmpty()
        if (id.isBlank()) {
            result.error("BAD_ARGUMENTS", "id 不能为空", null)
            return
        }
        val store = ensureInitialized()
        val credentialRef = store.getProvider(id)?.credentialRef?.takeIf { it.isNotBlank() }
        if (!store.deleteProvider(id)) {
            result.error("CONFIG_ERROR", "Provider 不存在: $id", null)
            return
        }
        if (credentialRef != null &&
            store.listProviders().none { it.credentialRef == credentialRef }
        ) {
            // 尽力而为：provider 已经删掉了，密钥清理失败不再把整个请求报成错误
            runCatching { credentials.remove(credentialRef) }
        }
        result.success(true)
    }

    /** `listModels(providerId)` → JSON 数组字符串；provider 不存在先报错（与桌面 `GET …/models` 同）。 */
    private fun listModels(call: MethodCall, result: MethodChannel.Result) {
        val providerId = call.argument<String>("providerId").orEmpty()
        val store = ensureInitialized()
        if (store.getProvider(providerId) == null) {
            result.error("CONFIG_ERROR", "Provider 不存在: $providerId", null)
            return
        }
        result.success(ProviderJson.modelsToJsonArray(store.listModels(providerId)))
    }

    /**
     * `saveModels(providerId, models: <JSON 数组字符串>)` → 保存后的整份目录（JSON 数组字符串）。
     *
     * **全量替换**（与桌面 `PUT /api/ai/providers/{id}/models` 同）：界面上的模型列表就是全部。
     */
    private fun saveModels(call: MethodCall, result: MethodChannel.Result) {
        val providerId = call.argument<String>("providerId").orEmpty()
        val raw = call.argument<String>("models").orEmpty()
        val elements = ProviderJson.parseArray(raw)
        if (elements == null) {
            result.error("BAD_ARGUMENTS", "models 必须是一段 JSON 数组", null)
            return
        }
        val objects = elements.mapNotNull { it as? JsonObject }
        if (objects.size != elements.size) {
            result.error("BAD_ARGUMENTS", "models 里每一项都必须是 JSON 对象", null)
            return
        }
        val store = ensureInitialized()
        val records = try {
            objects.map { ProviderJson.modelFromJson(it, providerId) }
        } catch (e: ProviderJson.ProviderJsonFailure) {
            result.error("BAD_ARGUMENTS", e.message, null)
            return
        }
        try {
            result.success(ProviderJson.modelsToJsonArray(store.saveModels(providerId, records)))
        } catch (e: ProviderStoreFailure) {
            result.error("CONFIG_ERROR", e.message, null)
        }
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
                    emitToSink(seq, type, "{}")
                }
                endSink()
                return@launch
            }
            for (event in channel) {
                emitToSink(event.seq, event.type, event.payload.toString())
            }
            endSink()
        }
    }

    /**
     * 往事件通道送一帧 —— **必须在主线程**。
     *
     * `EventChannel.EventSink` 是 `@UiThread` API（`FlutterJNI.dispatchPlatformMessage`
     * 里有 `ensureRunningOnMainThread()`）。以前这里在 `Dispatchers.Default` 的协程里
     * 直接 `sink.success(...)`，真机上**第一条事件就崩**：
     *
     * ```
     * FATAL EXCEPTION: DefaultDispatcher-worker-1
     * java.lang.RuntimeException: Methods marked with @UiThread must be executed on the main thread.
     *   at io.flutter.embedding.engine.FlutterJNI.ensureRunningOnMainThread
     *   at io.flutter.plugin.common.EventChannel$IncomingStreamRequestHandler$EventSinkImplementation.success
     *   at com.comfyhub.harness.bridge.HarnessBridge$startPump$1.invokeSuspend
     * ```
     *
     * 表现就是"**一发消息 App 就闪退**"。假 transport 的 JVM 单测抓不到它
     * （那条约束是 Android 运行时的），只有在真机上发第一条消息才看得见 ——
     * 2026-09-24 真机验收抓到。
     */
    private suspend fun emitToSink(seq: Int, type: String, payloadJson: String) {
        withContext(Dispatchers.Main) {
            sink?.success(mapOf("seq" to seq, "type" to type, "payloadJson" to payloadJson))
        }
    }

    /** 收尾同理：`endOfStream()` 也是 `@UiThread`。 */
    private suspend fun endSink() {
        withContext(Dispatchers.Main) { sink?.endOfStream() }
    }

    companion object {
        /** 方法通道名（Dart 侧 `MethodChannel('comfyhub/harness')`）。 */
        const val METHOD_CHANNEL = "comfyhub/harness"

        /** 事件通道名（Dart 侧 `EventChannel('comfyhub/harness/events')`）。 */
        const val EVENT_CHANNEL = "comfyhub/harness/events"

        /** 一条消息最多带几个卡片 / 技能引用（与桌面 `run_lifecycle::MAX_MENTIONS` 同值）。 */
        const val MAX_MENTIONS = 32
    }
}

