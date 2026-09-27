package com.comfyhub.harness.sync

import com.comfyhub.harness.net.HttpFailure
import com.comfyhub.harness.net.HttpTransport
import com.comfyhub.harness.store.HarnessStore
import com.comfyhub.harness.store.KvKey
import com.comfyhub.harness.store.OutboxKind
import com.comfyhub.harness.store.OutboxRecord
import com.comfyhub.harness.store.Timestamps
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

/**
 * 上行通道（**M4**，契约 🔒 `docs/sync-api-contract.md` §4）。
 *
 * 把手机 `_outbox` 里攒的东西推到桌面：`POST {base}/api/sync/push`
 * `{deviceId, batch:[{kind, refId, op:"upsert", payload:{完整行}}]}`，
 * 回 `{applied:[refId], rejected:[{refId, code, message}]}`。
 *
 * # 三条纪律（都是"不这么做就会静默出错"的那类）
 *
 * 1. **payload 是"推送时现读的完整行"，不是入队时的快照**。助手消息的正文是流式追加的、
 *    状态从 `streaming` 变 `complete`，入队那一刻的快照必然是半截的 —— 推上去桌面看到的
 *    是一条空回复，而两端都不报错。
 * 2. **永远不删"没被确认"的东西**：只有 `applied` 才清 outbox；
 *    `rejected` 要按**永久 / 暂时**分开处理（[isPermanentReject]）——
 *    把永久性失败当暂时重试，手机会永远卡在那一条上。
 * 3. **幂等是两端一起保证的**：桌面靠"主键 UUID + `ON CONFLICT DO UPDATE` + 服务端赢 LWW"，
 *    手机这边则**按 (kind, refId) 去重**（同一条本地行在 Run 期间可能入队好几次），
 *    重复推送不会产生重复行，但白推一遍会浪费一次往返、还会让后面几条落到 `STALE_IGNORED`。
 *
 * # 本类只做 HTTP + JSON
 *
 * 取行、清队列、记失败全部走 [HarnessStore] —— 这条分层是为了让"上行协议"能在宿主 JVM 上
 * 用一台假网关整条测出来（[com.comfyhub.harness.sync.SyncPushTest]）。
 */
class SyncPushClient(
    private val store: HarnessStore,
    private val transport: HttpTransport,
) {

    /**
     * 推一批（默认最多 200 条；桌面单批上限 500，超了会整批 400）。
     *
     * 一次调用只做**一批**：这样"还剩多少"永远是真实读数，
     * 调用方（桥 / 前台任务）想推完就循环调，中间可以被打断而不会丢东西。
     */
    fun push(baseUrl: String, token: String?, limit: Int = 200): SyncOutcome {
        val pending = store.pendingOutbox(limit.coerceIn(1, MAX_BATCH))
        if (pending.isEmpty()) return SyncOutcome.empty()

        // ① 组装 batch：按 (kind, refId) 去重；本地行已经没了的直接**清掉**那几条 outbox
        //    （它指向的东西不存在了，留在队列里只会每次重试都被拒）
        val seen = mutableSetOf<String>()
        val items = mutableListOf<OutboxRecord>()
        var droppedLocal = 0
        for (record in pending) {
            val key = "${record.kind}\u0000${record.refId}"
            if (!seen.add(key)) {
                store.removeOutbox(record.id) // 同一行的重复入队：合成一条
                continue
            }
            if (store.outboxTarget(record.kind, record.refId) == null) {
                // `run` / `submit_job` 还没有上行通道（M5）→ **跳过但不清**（清了就是悄悄丢东西）。
                // 其余两种情况都清掉：① 本地那行已经没了（推送前被删了）；
                // ② `ai_run` / `ai_tool_call` 指向**桌面同步下来的**行 —— 入队那三处都判过
                //    `local_runs`，真漏进来一条也是"这条本来就不该推"，留着只会每次白重试。
                if (record.kind == OutboxKind.RUN || record.kind == OutboxKind.SUBMIT_JOB) continue
                store.removeOutbox(record.id)
                droppedLocal++
                continue
            }
            items += record
        }
        if (items.isEmpty()) {
            return SyncOutcome(applied = 0, rejectedPermanent = 0, rejectedTransient = 0, pending = store.pendingOutbox(limit).size, droppedLocal = droppedLocal)
        }

        val batch = buildJsonArray {
            for (record in items) {
                val target = store.outboxTarget(record.kind, record.refId) ?: continue
                add(
                    buildJsonObject {
                        put("kind", target.wireKind)
                        put("refId", record.refId)
                        put("op", "upsert")
                        put("payload", rowToJson(target.payload))
                    },
                )
            }
        }
        val body = buildJsonObject {
            put("deviceId", deviceId())
            put("batch", batch)
        }.toString()

        val reply = try {
            transport.postJson(
                baseUrl.trimEnd('/') + PUSH_PATH,
                body,
                buildMap {
                    put("Content-Type", "application/json")
                    if (!token.isNullOrBlank()) put("X-ComfyHub-Token", token)
                },
            )
        } catch (e: HttpFailure) {
            // 网络层失败 = **暂时**：一条都不许丢，全部记一次失败等下次
            val message = "上行失败：${e.message ?: e.status?.toString() ?: "网络不可达"}"
            items.forEach { store.markOutboxFailed(it.id, message) }
            return SyncOutcome(
                applied = 0,
                rejectedPermanent = 0,
                rejectedTransient = items.size,
                pending = store.pendingOutbox(limit).size,
                error = message,
            )
        }
        if (!reply.ok) {
            val message = "上行失败：HTTP ${reply.status} ${reply.body.take(200)}"
            items.forEach { store.markOutboxFailed(it.id, message) }
            return SyncOutcome(
                applied = 0,
                rejectedPermanent = 0,
                rejectedTransient = items.size,
                pending = store.pendingOutbox(limit).size,
                error = message,
            )
        }

        // ② 认回执：`applied` 才清；`rejected` 按永久/暂时分开
        val parsed = try {
            Json.parseToJsonElement(reply.body).jsonObject
        } catch (e: Throwable) {
            // 回执读不懂 = 不知道发生了什么 → 当**暂时**失败（宁可重推，也不能假装成功）
            val message = "上行回执读不懂：${e.message}"
            items.forEach { store.markOutboxFailed(it.id, message) }
            return SyncOutcome(0, 0, items.size, store.pendingOutbox(limit).size, error = message)
        }
        val applied = parsed["applied"]?.jsonArray?.map { it.jsonPrimitive.content }?.toSet().orEmpty()
        val rejected = parsed["rejected"]?.jsonArray?.map { it.jsonObject }.orEmpty()

        var okCount = 0
        var permanent = 0
        var transient = 0
        for (record in items) {
            if (record.refId in applied) {
                store.removeOutbox(record.id)
                okCount++
                continue
            }
            val reject = rejected.firstOrNull { it["refId"]?.jsonPrimitive?.content == record.refId }
            if (reject == null) {
                // 既没说成功、也没说被拒 —— 桌面**漏回了**。当暂时失败，绝不当作成功丢掉
                store.markOutboxFailed(record.id, "桌面回执里没有这一条（既未 applied 也未 rejected）")
                transient++
                continue
            }
            val code = reject["code"]?.jsonPrimitive?.content.orEmpty()
            val message = reject["message"]?.jsonPrimitive?.content.orEmpty()
            if (isPermanentReject(code)) {
                // 永久性失败：**丢掉**（留着只会每次重推每次被拒）
                store.removeOutbox(record.id)
                permanent++
            } else {
                store.markOutboxFailed(record.id, "$code：$message")
                transient++
            }
        }

        return SyncOutcome(
            applied = okCount,
            rejectedPermanent = permanent,
            rejectedTransient = transient,
            pending = store.pendingOutbox(limit).size,
            droppedLocal = droppedLocal,
        )
    }

    /** 本机设备 id：读一次、没有就生成一次并落库（生成两次 = 桌面当成两台设备）。 */
    fun deviceId(): String {
        store.getKv(KvKey.DEVICE_ID)?.takeIf { it.isNotBlank() }?.let { return it }
        val generated = "phone-" + Timestamps.newId()
        store.putKv(KvKey.DEVICE_ID, generated)
        return generated
    }

    companion object {
        /** 与桌面 `POST /api/sync/push` 同路径（改了要同时改两边）。 */
        const val PUSH_PATH = "/api/sync/push"

        /** 桌面单批上限（契约 §4）：超了**整批 400**，所以本地就要夹住。 */
        const val MAX_BATCH = 500

        /**
         * 这个错误码是不是**永久性**的（手机据此把 outbox 那行丢掉）。
         *
         * 判据只有一条：**重推一次结果会不一样吗**。不会变的一律永久：
         * `UNKNOWN_KIND` / `UNSUPPORTED_OP` / `BAD_PAYLOAD` / `UNKNOWN_TABLE` /
         * `SCHEMA_MISMATCH` / `FORBIDDEN_KIND`（总纲 §3.3：手机不许创建那几张表）、
         * `STALE_IGNORED`（LWW 输了 —— 桌面那份更新，再推一万次还是输）。
         * 只有 `DB_ERROR` 是暂时的（桌面那一刻落库失败，重试有意义）。
         *
         * ⚠ 未知的码按**永久**处理还是暂时？这里选**永久**：桌面新增一个码多半意味着
         * "这条数据形状就是不对"，重试只会让手机永远卡在队首；而暂时性的东西
         * （网络、锁）本来就会走 `HttpFailure` / `DB_ERROR` 那两条路。
         */
        fun isPermanentReject(code: String): Boolean = code != "DB_ERROR"
    }
}

/** 一次上行的结果（给人的那种"推了几条、还剩几条"）。 */
data class SyncOutcome(
    val applied: Int,
    val rejectedPermanent: Int,
    val rejectedTransient: Int,
    val pending: Int,
    /**
     * 本地那行已经没了（顺手清掉）、或这条本就**不该推**（桌面同步下来的 Run / 工具明细）
     * 的条数。两种都是"清掉是对的"，所以合成一个计数。
     */
    val droppedLocal: Int = 0,
    val error: String? = null,
) {
    val touched: Int get() = applied + rejectedPermanent + rejectedTransient

    companion object {
        fun empty() = SyncOutcome(0, 0, 0, 0)
    }
}

/**
 * 一行（列名 → 值）→ JSON 对象。
 *
 * **NULL 直接不发**：可空列不发就等于"这条不覆盖那一列"，桌面保留自己那份 ——
 * 推一个 `"provider_id": null` 上去反而会把桌面上已经有的值抹掉。
 * 而 NOT NULL 的列本地一定有值（镜像表与桌面逐列同形），不会因此漏。
 */
private fun rowToJson(row: Map<String, Any?>): JsonObject = buildJsonObject {
    for ((column, value) in row) {
        when (value) {
            null -> Unit
            is Boolean -> put(column, if (value) 1 else 0)
            is Number -> put(column, value)
            is String -> put(column, value)
            // 这几张镜像表没有 BLOB 列；真出现了也不推（推一个错的形状比不推更糟）
            else -> Unit
        }
    }
}
