package com.comfyhub.harness.tools

import com.comfyhub.harness.net.HttpTransport
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

/**
 * 桌面上那份 skill 的**只读**客户端（[RemoteSkills] 的真实现）。
 *
 * 补上"手机本地只有随包那几份、桌面上的都取不到"那个洞：
 * `load_skill` 先查本机投放口，没有再走这里；`list_skills` 靠 [names] 把两边并起来。
 *
 * # 三条纪律
 *
 * 1. **只发 GET**：注册 / 删除 / 启停都是桌面独有的事，手机不解这摊；
 * 2. **"连不上"与"没有"分开**（见 [RemoteSkillBody] 的三态）：`baseUrl` 还没拿到、
 *    或者网络异常，一律 `Unreachable` —— 说成"没有"会让用户以为 skill 不存在；
 * 3. **不缓存正文**：桌面随时会改，一次工具调用取一次就够。
 *
 * `baseUrl` / `token` 由平台桥在每次"连桌面"的动作里更新（取 run-kit / 上行 / 下行
 * 那三处本来就要连桌面，顺手记下来，不另开一个"桌面地址"的来源）。
 *
 * 放在 `:harness` 而不是 app 模块：它只用 [HttpTransport] 与 JSON，不碰任何 Android API，
 * 所以这些响应形状（与桌面 `GET /api/ai/skills*` 是一份契约）能用 JVM 单测钉住。
 */
class DesktopSkills(private val transport: HttpTransport) : RemoteSkills {

    /** 桌面地址（空 = 还没连过）。 */
    @Volatile
    var baseUrl: String = ""

    /** 配对令牌（闸门打开时必带；没配对就是 null）。 */
    @Volatile
    var token: String? = null

    override suspend fun names(): List<RemoteSkill>? {
        val base = normalizedBase() ?: return null
        val reply = runCatching { transport.get("$base/api/ai/skills", headers()) }.getOrNull()
            ?: return null
        if (reply.status !in 200..299) return null
        val array = runCatching { Json.parseToJsonElement(reply.body).jsonArray }.getOrNull()
            ?: return null
        return array.mapNotNull { element ->
            val obj = runCatching { element.jsonObject }.getOrNull() ?: return@mapNotNull null
            val name = obj["name"]?.jsonPrimitive?.contentOrNull.orEmpty()
            if (name.isEmpty()) {
                null
            } else {
                RemoteSkill(
                    name = name,
                    description = obj["description"]?.jsonPrimitive?.contentOrNull.orEmpty(),
                    version = obj["version"]?.jsonPrimitive?.contentOrNull,
                )
            }
        }
    }

    override suspend fun body(name: String): RemoteSkillBody {
        val base = normalizedBase() ?: return RemoteSkillBody.Unreachable
        val url = "$base/api/ai/skills/" + java.net.URLEncoder.encode(name, "UTF-8")
        val reply = runCatching { transport.get(url, headers()) }.getOrNull()
            ?: return RemoteSkillBody.Unreachable
        // 桌面对"没有这一份"回的是 400 CONFIG_ERROR（`ai_skill_detail` 那条），也认 404
        if (reply.status == 400 || reply.status == 404) return RemoteSkillBody.Missing
        if (reply.status !in 200..299) return RemoteSkillBody.Unreachable
        val obj = runCatching { Json.parseToJsonElement(reply.body).jsonObject }.getOrNull()
            ?: return RemoteSkillBody.Unreachable
        val content = obj["content"]?.jsonPrimitive?.contentOrNull
        return if (content.isNullOrBlank()) RemoteSkillBody.Missing else RemoteSkillBody.Found(content)
    }

    private fun normalizedBase(): String? =
        baseUrl.trim().trimEnd('/').takeIf { it.isNotEmpty() }

    private fun headers(): Map<String, String> =
        token?.takeIf { it.isNotBlank() }?.let { mapOf("X-ComfyHub-Token" to it) } ?: emptyMap()
}
