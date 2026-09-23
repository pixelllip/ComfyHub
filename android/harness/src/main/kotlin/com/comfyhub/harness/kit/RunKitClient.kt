package com.comfyhub.harness.kit

import com.comfyhub.harness.net.HttpTransport
import com.comfyhub.harness.store.HarnessStore
import com.comfyhub.harness.store.KvKey
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

/**
 * 从桌面取 run-kit / 查吊销 —— `GET /api/ai/run-kit{,/status}`（🔒 `docs/run-kit-contract.md` §1）。
 *
 * ⚠ **规则由桌面产出**：手机不生成 kit、不改 kit，只负责取、缓存、比对版本。
 */
class RunKitClient(
    private val transport: HttpTransport,
    private val baseUrl: String,
    private val token: String? = null,
) {

    private fun headers(): Map<String, String> =
        if (token.isNullOrBlank()) emptyMap() else mapOf("X-ComfyHub-Token" to token)

    /**
     * 取一份 kit。
     *
     * `?providerId=&modelId=` 让手机按它选好的模型取**环境头**（系统提示里写着当前模型的能力，
     * 见契约 §5 的实现备注）—— 少了参数就用桌面的当前默认。
     */
    fun fetch(providerId: String? = null, modelId: String? = null): RunKit {
        val query = buildList {
            if (!providerId.isNullOrBlank()) add("providerId=$providerId")
            if (!modelId.isNullOrBlank()) add("modelId=$modelId")
        }.joinToString("&")
        val url = baseUrl.trimEnd('/') + "/api/ai/run-kit" + if (query.isEmpty()) "" else "?$query"
        val reply = transport.get(url, headers()).requireOk()
        return RunKitCodec.parse(reply.body)
    }

    /** 吊销查询：`{ current, revoked: [...] }`。 */
    fun status(): KitStatus {
        val reply = transport.get(baseUrl.trimEnd('/') + "/api/ai/run-kit/status", headers()).requireOk()
        val root = runCatching {
            com.comfyhub.harness.protocol.ProtocolJson.parseToJsonElement(reply.body) as? JsonObject
        }.getOrNull() ?: throw RunKitCodec.KitParseFailure("run-kit/status 的响应不是 JSON 对象")
        return KitStatus(
            current = (root["current"] as? JsonPrimitive)?.contentOrNull,
            revoked = (root["revoked"] as? JsonArray)
                ?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
                .orEmpty(),
        )
    }
}

/** `/run-kit/status` 的响应。 */
data class KitStatus(val current: String?, val revoked: List<String>)

/**
 * kit 的本地缓存（存在 `mobile_kv` 里，跟着本地库走）。
 *
 * 为什么缓存是**需求**而不是优化：B 方案的价值就是"**桌面不在线**也能继续出方案"
 * （`docs/mobile-harness-design.md` §0/§9.2）。没有缓存的话，桌面一离线手机连系统提示都拿不到。
 */
class RunKitCache(private val store: HarnessStore) {

    fun save(kit: RunKit) {
        store.putKv(KEY_KIT, RunKitCodec.toJson(kit))
        store.putKv(KvKey.KIT_VERSION, kit.kitVersion)
        store.putKv(KvKey.SCHEMA_VERSION, kit.schemaVersion.toString())
    }

    fun load(): RunKit? = store.getKv(KEY_KIT)?.let { raw ->
        runCatching { RunKitCodec.parse(raw) }.getOrNull() // 缓存坏了就当作没有，重新取
    }

    fun cachedVersion(): String? = store.getKv(KvKey.KIT_VERSION)

    fun cachedSchemaVersion(): Int? = store.getKv(KvKey.SCHEMA_VERSION)?.toIntOrNull()

    companion object {
        const val KEY_KIT = "runKit.payload"
    }
}

/**
 * "现在能不能开跑"的判定（🔒 契约 §1 的吊销语义）。
 *
 * 三种结果：
 *  - [Decision.Run]：手里的 kit 可用（可能有 [Decision.Run.warning]，比如离线没法核对吊销）；
 *  - [Decision.Refresh]：必须**联网重取**（命中吊销 / 版本已经不是当前那份）；
 *  - [Decision.Blocked]：连缓存的 kit 都没有 —— 没规则就不许跑（宁可不跑，也不给模型一套瞎编的规则）。
 */
sealed interface Decision {
    data class Run(val kit: RunKit, val warning: String? = null) : Decision
    data class Refresh(val reason: String) : Decision
    data object Blocked : Decision
}

object RunKitGuard {

    /**
     * @param cached 本地缓存的那份（没有就是 null）
     * @param status 刚查到的 `/run-kit/status`；**离线时为 null**（查不通不该拦住用户）
     */
    fun decide(cached: RunKit?, status: KitStatus?): Decision {
        if (cached == null) {
            return if (status == null) {
                Decision.Blocked
            } else {
                Decision.Refresh("本地还没有规则工件（run-kit），必须联网取一次")
            }
        }
        if (status == null) {
            // 离线：用缓存继续（B 方案的定义就是"桌面不在线"），但要如实告诉调用方"没核对过吊销"
            return Decision.Run(cached, warning = "离线：未能核对 run-kit 吊销状态")
        }
        if (cached.kitVersion in status.revoked) {
            return Decision.Refresh("当前规则版本 ${cached.kitVersion} 已被吊销，必须联网同步后再继续")
        }
        val current = status.current
        if (!current.isNullOrBlank() && current != cached.kitVersion) {
            return Decision.Refresh("桌面已有更新的规则版本（$current ≠ ${cached.kitVersion}）")
        }
        return Decision.Run(cached)
    }
}
