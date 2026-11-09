package com.comfyhub.harness.store

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put

/**
 * **`submit_job` 的摊平**（契约 🔒 `docs/sync-api-contract.md` §4.1）。
 *
 * `submit_job` **不是"一张表的行"，是一条指令**（"请桌面把这条工作流真的提交一次"）：
 * 桌面 `push` 里单独一个分支受理它（`submit_queue::accept`），
 * `refId` = 手机生成的 `queueId`（幂等键）。
 *
 * 线上形状是**扁平的、键名固定**：
 *
 * ```json
 * { "queueId": "…", "conversationId": "…", "runId": "…",
 *   "promptId": 310, "sourceRef": "file:<sha256>",
 *   "task": "t2i", "modelFamily": "anima", "positivePrompt": "…", "width": 832, "height": 1216,
 *   "overrides": { "6.text": "…" }, "nsfw": false,
 *   "requestedAt": "…Z", "requestedBy": "phone-7f3a…" }
 * ```
 *
 * ## 两条"给它工作流"的路（2026-11-09 起）
 *
 * 1. **精确指定**：`promptId`（数字）或 `sourceRef`（`file:<sha256>` 那种运行键）；
 * 2. **只说族**：`task` + `modelFamily` + `positivePrompt` —— 桌面按"同任务同族的最近成功 →
 *    已验证的配方"自己挑一份，把提示词写进去再跑（`docs/workflow-auto-select-plan.md` §1.2）。
 *    手机**看不到**桌面的工作流库，所以"只说族"是床上场景唯一说得清的说法。
 *
 * ## ⚠ 为什么必须摊平（这是最容易踩的一条）
 *
 * 手机侧的工具拿到的是**自由参数**（`{"workflow": …, "prompt": …}` 那类），而桌面只认上面
 * 那套顶层键。整包塞在一个 `args` 子对象里推上去，桌面会按**永久性** `MISSING_WORKFLOW` 拒
 * （它不可能重试成功），而手机把永久失败那行 outbox **丢掉** ——
 * 用户看到的是"我点了提交，**什么都没发生**"。所以摊平这件事没有任何余地。
 *
 * ## 自由文本 `prompt` 现在**不再拒**（2026-11-09）
 *
 * 它会被当作 `positivePrompt` 摊平出去：桌面会把最终提示词写进工作流（`positivePrompt` 那条路
 * 会找出正面编码器，并把 TextGenerate 那一支从这次提交的图里摘掉）。**手机不扩写、不改写**——
 * 模型写什么就传什么，写错了在回执里看得见。
 */
object SubmitJobPayload {

    /** 摊平的结果：要么是线上形状，要么是一句**人话**的原因（给模型看）。 */
    sealed interface Flattened {
        data class Ok(val payload: JsonObject) : Flattened
        data class Bad(val reason: String) : Flattened
    }

    /** 工具的**自由参数**（键名刻意宽容一点：模型爱怎么叫就怎么叫，只要意图明确）。 */
    private const val ARG_PROMPT_ID = "promptId"
    private const val ARG_SOURCE_REF = "sourceRef"
    private const val ARG_WORKFLOW = "workflow"
    private const val ARG_WORKFLOW_ID = "workflowId"
    private const val ARG_OVERRIDES = "overrides"
    private const val ARG_NSFW = "nsfw"
    private const val ARG_PROMPT = "prompt"

    // ---- 2026-11-09：只说族那一路（床上场景 §1.2）----
    private const val ARG_TASK = "task"
    private const val ARG_MODEL_FAMILY = "modelFamily"
    private const val ARG_POSITIVE_PROMPT = "positivePrompt"
    private const val ARG_WIDTH = "width"
    private const val ARG_HEIGHT = "height"

    /** 宽高的合理区间（与桌面 `submit_queue::parse_payload` 同一条判据）。 */
    private const val MIN_SIZE = 64L
    private const val MAX_SIZE = 8192L

    /**
     * 把一次 `comfy_queue_submit` 的意图摊平成线上形状。
     *
     * @param requestedAt 排队那一刻（`queuedAt` 的线上名）
     * @param runId 发起这次排队的本机 Run（可以为 null —— 桌面把它记在账上，不靠它找人）
     */
    fun flatten(
        args: JsonObject,
        queueId: String,
        conversationId: String,
        runId: String?,
        requestedAt: String,
    ): Flattened {
        if (queueId.isBlank()) return Flattened.Bad("queueId 为空（这是本机的 bug，请如实告诉用户）")

        // ---- 哪条工作流：promptId（数字）或 sourceRef（`file:<sha256>` 那种运行键）----
        var promptId = intOf(args[ARG_PROMPT_ID])
        var sourceRef = textOf(args[ARG_SOURCE_REF])
        val rawWorkflow = textOf(args[ARG_WORKFLOW]) ?: textOf(args[ARG_WORKFLOW_ID])
        if (promptId == null && sourceRef == null && rawWorkflow != null) {
            // `workflow` 这一格里可能是数字 id，也可能已经是运行键（`file:<sha256>` / `run:<…>`）
            val asNumber = rawWorkflow.toLongOrNull()
            if (asNumber != null) {
                promptId = asNumber
            } else if (rawWorkflow.contains(':')) {
                sourceRef = rawWorkflow
            }
        }

        // ---- 只说族那一路：task / modelFamily / positivePrompt（可选宽高）----
        val task = textOf(args[ARG_TASK])?.lowercase()
        val family = textOf(args[ARG_MODEL_FAMILY])
        val positivePrompt = textOf(args[ARG_POSITIVE_PROMPT]) ?: textOf(args[ARG_PROMPT])
        val width = intOf(args[ARG_WIDTH])
        val height = intOf(args[ARG_HEIGHT])
        for ((key, value) in listOf("width" to width, "height" to height)) {
            if (value != null && (value < MIN_SIZE || value > MAX_SIZE)) {
                return Flattened.Bad("$key=$value 不合理：要在 $MIN_SIZE~$MAX_SIZE 之间（像素）")
            }
        }
        // ⚠ **只有提示词不算"说清了要哪份工作流"**：族 / 任务才是选择依据。
        //   拿提示词当判据会让"只说提示词"变成"随便挑最近那份"——那正是"不猜"要挡住的东西。
        val canPick = task != null || family != null

        if (promptId == null && sourceRef == null && !canPick) {
            return Flattened.Bad(
                "不知道该提交哪条工作流：要么给我 `promptId`（提示词 id，数字）或 " +
                    "`sourceRef`（运行键，形如 `file:<sha256>`），要么给 `task` + `modelFamily` " +
                    "（让桌面按\"同任务同族的最近成功 → 已验证的配方\"自己挑）。" +
                    "只给一个工作流名字不行 —— 手机这边没有那份工作流库（它由桌面产生），" +
                    "名字解析不出 id。" +
                    (if (rawWorkflow != null) "（收到的是 workflow=\"$rawWorkflow\"）" else ""),
            )
        }
        if (promptId == null && sourceRef != null && !sourceRef.contains(':')) {
            // 显式给了 sourceRef：形状要能认（`<方案>:<值>`），否则大概率塞错了东西
            return Flattened.Bad(
                "sourceRef 的形状不对（收到 \"$sourceRef\"）：它要是运行键，形如 `file:<sha256>`；" +
                    "或者直接给 `promptId`（数字）。",
            )
        }

        // ---- overrides：只认对象（键 = `节点id.输入名`）----
        val rawOverrides = args[ARG_OVERRIDES]
        val overrides = when {
            rawOverrides == null || rawOverrides is kotlinx.serialization.json.JsonNull -> null
            rawOverrides is JsonObject -> rawOverrides
            else -> return Flattened.Bad(
                "overrides 必须是对象（键 = `节点id.输入名`，例如 {\"6.text\": \"赛博朋克\"}）；" +
                    "收到的是别的东西。",
            )
        }

        return Flattened.Ok(
            buildJsonObject {
                put("queueId", queueId)
                put("conversationId", conversationId)
                runId?.takeIf { it.isNotBlank() }?.let { put("runId", it) }
                promptId?.let { put("promptId", it) }
                sourceRef?.takeIf { it.isNotBlank() }?.let { put("sourceRef", it) }
                // 只说族那一路（与精确指定**可以同时给**：精确指定优先，桌面不会用族覆盖它）
                task?.let { put(ARG_TASK, it) }
                family?.let { put(ARG_MODEL_FAMILY, it) }
                positivePrompt?.let { put(ARG_POSITIVE_PROMPT, it) }
                width?.let { put(ARG_WIDTH, it) }
                height?.let { put(ARG_HEIGHT, it) }
                // 空对象与"没给"等价（桌面 `accept` 里 overrides 空 = 不改任何参数），省掉这个键
                overrides?.takeIf { it.isNotEmpty() }?.let { put("overrides", it) }
                // `nsfw` 永远写出来（契约 §4.1 有这个键；缺省 = false，与桌面 `accept` 同义）
                put("nsfw", boolOf(args[ARG_NSFW]) ?: false)
                put("requestedAt", requestedAt)
            },
        )
    }

    /**
     * **推送那一刻**：把 `_outbox` 里存的那份 payload 整理成线上形状。
     *
     * 两种输入都要认：
     *  - **新形状**（本批起工具直接摊平存的）：补上 `queueId` / `requestedBy` 就用；
     *  - **老形状**（`{runId, conversationId, args, queuedAt}`，本批之前排下的队）：
     *    用它里面的 `args` 再摊一次 —— 老队列在真机上确实存在（那时 `outboxTarget` 对
     *    `submit_job` 回 null，所以那些行**一直躺在队列里没推出去**）。
     *
     * 摊不平就回 `null`：那行 outbox **不该推**（推上去必然被永久拒，然后被我们自己丢掉）。
     * 但"进队列"这一步已经被工具挡住（[com.comfyhub.harness.tools.ComfyQueueSubmitTool]
     * 摊不平就**不排队**、当场告诉模型），所以走到这里的 `null` 只有一种来源 —— 老队列。
     *
     * @param requestedBy 设备号（`sync.deviceId`）——有就带上，桌面账上那一列是给人看的
     */
    fun forPush(storedPayloadJson: String?, refId: String, requestedBy: String?): JsonObject? {
        val stored = parseObject(storedPayloadJson) ?: return null
        val queueId = refId.trim().ifEmpty { textOf(stored["queueId"]) ?: return null }
        val conversationId = textOf(stored["conversationId"]) ?: return null

        // 老形状：`args` 子对象里才是真正的意图
        val legacyArgs = stored["args"] as? JsonObject
        if (legacyArgs != null) {
            val runId = textOf(stored["runId"])
            val requestedAt = textOf(stored["requestedAt"]) ?: textOf(stored["queuedAt"]) ?: Timestamps.now()
            val flattened = flatten(legacyArgs, queueId, conversationId, runId, requestedAt)
            if (flattened is Flattened.Bad) return null
            val payload = (flattened as Flattened.Ok).payload
            return withRequestedBy(payload, requestedBy)
        }

        // 新形状：只补两个"推送时才定得下来"的键
        val hasAuto = textOf(stored[ARG_TASK]) != null ||
            textOf(stored[ARG_MODEL_FAMILY]) != null ||
            textOf(stored[ARG_POSITIVE_PROMPT]) != null
        if (intOf(stored["promptId"]) == null && textOf(stored["sourceRef"]) == null && !hasAuto) return null
        return withRequestedBy(
            buildJsonObject {
                put("queueId", queueId)
                put("conversationId", conversationId)
                textOf(stored["runId"])?.let { put("runId", it) }
                intOf(stored["promptId"])?.let { put("promptId", it) }
                textOf(stored["sourceRef"])?.let { put("sourceRef", it) }
                textOf(stored[ARG_TASK])?.let { put(ARG_TASK, it) }
                textOf(stored[ARG_MODEL_FAMILY])?.let { put(ARG_MODEL_FAMILY, it) }
                textOf(stored[ARG_POSITIVE_PROMPT])?.let { put(ARG_POSITIVE_PROMPT, it) }
                intOf(stored[ARG_WIDTH])?.let { put(ARG_WIDTH, it) }
                intOf(stored[ARG_HEIGHT])?.let { put(ARG_HEIGHT, it) }
                (stored["overrides"] as? JsonObject)?.takeIf { it.isNotEmpty() }?.let { put("overrides", it) }
                put("nsfw", boolOf(stored["nsfw"]) ?: false)
                put("requestedAt", textOf(stored["requestedAt"]) ?: Timestamps.now())
            },
            requestedBy,
        )
    }

    private fun withRequestedBy(payload: JsonObject, requestedBy: String?): JsonObject {
        if (requestedBy.isNullOrBlank() || payload.containsKey("requestedBy")) return payload
        return buildJsonObject {
            payload.forEach { (k, v) -> put(k, v) }
            put("requestedBy", requestedBy)
        }
    }

    // -----------------------------------------------------------------------
    //  JSON → **普通 Kotlin 值**
    // -----------------------------------------------------------------------

    /**
     * 摊平出来的 [JsonObject] → `Map<String, Any?>`（上行那一路要的**普通值**）。
     *
     * ⚠ 为什么必须转（这一条踩过）：`SyncPush.rowToJson` 只认 null / Boolean / Number / String，
     * 别的类型**整键丢掉**。直接把 `JsonObject` 递给它，`overrides` 那种嵌套对象会被静默丢掉
     * （桌面受理了、用户的参数改动却没生效），而 `queueId` 之类的 `JsonPrimitive` 更是会
     * 连键一起没 —— 表现是桌面回 `BAD_PAYLOAD`，而手机这边看不出哪里错了。
     */
    fun toPlainMap(payload: JsonObject): Map<String, Any?> = payload.mapValues { (_, v) -> plain(v) }

    private fun plain(value: kotlinx.serialization.json.JsonElement?): Any? = when (value) {
        null -> null
        is kotlinx.serialization.json.JsonNull -> null
        is JsonPrimitive -> when {
            value.isString -> value.content
            value.content == "true" -> true
            value.content == "false" -> false
            else -> value.content.toLongOrNull() ?: value.content.toDoubleOrNull() ?: value.content
        }
        is JsonObject -> value.mapValues { (_, v) -> plain(v) }
        is kotlinx.serialization.json.JsonArray -> value.map { plain(it) }
    }

    // -----------------------------------------------------------------------
    //  JSON 取值（**宽容读入**：模型给的 JSON 不保证类型正确）
    // -----------------------------------------------------------------------

    private fun parseObject(text: String?): JsonObject? {
        if (text.isNullOrBlank()) return null
        return runCatching {
            com.comfyhub.harness.protocol.ProtocolJson.parseToJsonElement(text) as? JsonObject
        }.getOrNull()
    }

    private fun textOf(value: kotlinx.serialization.json.JsonElement?): String? =
        (value as? JsonPrimitive)?.contentOrNull?.trim()?.takeIf { it.isNotEmpty() }

    private fun intOf(value: kotlinx.serialization.json.JsonElement?): Long? =
        textOf(value)?.toLongOrNull()

    private fun boolOf(value: kotlinx.serialization.json.JsonElement?): Boolean? =
        textOf(value)?.toBooleanStrictOrNull()
}
