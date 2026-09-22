package com.comfyhub

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.nio.file.Files
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * **端口状态文件**（用户 2026-09-22："不再显式指定端口，启动时自动挑一个"）。
 *
 * 这个文件是端口这件事的**唯一真源**：后端写，前端（`settings_store.dart`）、
 * 脚本（`scripts\api-port.ps1`）、ComfyUI 捕获扩展（`capture_core.py`）都读它。
 * 所以这组用例钉的不是"写得对不对"，而是**三个读端各自在意的那几条**：
 *
 *  · 文件能被**非 Kotlin** 的读端读懂（键名固定、是纯 JSON、没有反序列化才懂的花样）；
 *  · 坏文件 / 半截文件**绝不抛异常**（读端宁愿"这次读不到、下次再读"，也不能崩）；
 *  · 清理只清**自己写的那一份**（重启场景：新进程刚写好的文件不能被旧进程的收尾删掉）。
 */
class ApiPortFileTest {

    private fun tempRoot(): java.nio.file.Path =
        Files.createTempDirectory("comfyhub-port").toRealPath()

    private fun state(
        port: Int = 51234,
        pid: Long = 4242,
        requested: Int = 0,
        fellBack: Boolean = false,
    ) = ApiPortState(
        port = port,
        pid = pid,
        host = "127.0.0.1",
        url = ApiPortFile.urlFor("127.0.0.1", port),
        startedAt = "2026-09-22T09:00:00Z",
        requestedPort = requested,
        fellBack = fellBack,
    )

    @Test
    fun `写进去再读出来 端口与来源都一致`() {
        val root = tempRoot()
        ApiPortFile.write(root, state(port = 51234, pid = 77, requested = 8280, fellBack = true))

        val read = ApiPortFile.read(root)!!
        assertEquals(51234, read.port)
        assertEquals(77, read.pid)
        assertEquals("http://127.0.0.1:51234", read.url)
        assertEquals(8280, read.requestedPort, "要能说清'你给的 8280 被占了'")
        assertTrue(read.fellBack)
    }

    @Test
    fun `是纯 JSON 且键名固定——前端 脚本 Python 扩展都按这几个键读`() {
        val root = tempRoot()
        ApiPortFile.write(root, state(port = 43210))
        val text = Files.readString(ApiPortFile.path(root))
        val obj = AppJson.parseToJsonElement(text) as JsonObject
        // 少一个键 -> 三个读端各写一套兜底，等于把"唯一真源"又拆开了
        listOf("port", "pid", "host", "url", "startedAt", "requestedPort", "fellBack").forEach { key ->
            assertTrue(obj[key] is JsonPrimitive, "状态文件缺键 $key：$text")
        }
        assertEquals(43210, obj["port"]!!.jsonPrimitive.int)
    }

    @Test
    fun `文件不存在 或内容是半截 JSON 时 读出来是 null 而不是抛异常`() {
        val root = tempRoot()
        assertNull(ApiPortFile.read(root), "没有文件就是没有，不要抛")

        // 半截 JSON（模拟"读到了写入过程中的文件"—— 写侧是原子替换，理论上不该发生，
        // 但读端必须扛得住，否则 App 启动页会因为这个崩一次）
        val target = ApiPortFile.path(root)
        Files.createDirectories(target.parent)
        Files.writeString(target, """{"port": 51""")
        assertNull(ApiPortFile.read(root))

        Files.writeString(target, "[]")
        assertNull(ApiPortFile.read(root), "类型不对也算读不到")
    }

    @Test
    fun `清理只删自己写的那一份`() {
        val root = tempRoot()
        // 场景：用户点「重启」，新后端(pid 200) 已经写好文件，旧后端(pid 100) 的收尾才跑完
        ApiPortFile.write(root, state(port = 50001, pid = 200))

        assertFalse(ApiPortFile.deleteIfOwned(root, 100), "pid 不匹配就不该删")
        assertEquals(50001, ApiPortFile.read(root)!!.port, "新进程的记录必须还在")

        assertTrue(ApiPortFile.deleteIfOwned(root, 200), "是自己的就该清掉")
        assertNull(ApiPortFile.read(root))
    }

    @Test
    fun `重复写入是覆盖 不是追加（重启后读到的是新端口）`() {
        val root = tempRoot()
        ApiPortFile.write(root, state(port = 40001))
        ApiPortFile.write(root, state(port = 40002))
        assertEquals(40002, ApiPortFile.read(root)!!.port)
        // 临时文件不许残留：它会跟着 `.run` 一起被 commit 或被打包
        assertFalse(Files.exists(ApiPortFile.path(root).resolveSibling("${ApiPortFile.FILE_NAME}.tmp")))
    }

    @Test
    fun `url 拼装：通配地址回落回环 端口必须带上`() {
        // `COMFYHUB_HOST=0.0.0.0` 时，"连自己"不能用 0.0.0.0（本机连不上）
        assertEquals("http://127.0.0.1:5555", ApiPortFile.urlFor("0.0.0.0", 5555))
        assertEquals("http://127.0.0.1:5555", ApiPortFile.urlFor("", 5555))
        assertEquals("http://192.168.1.9:5555", ApiPortFile.urlFor("192.168.1.9", 5555))
        assertEquals("http://[::1]:5555", ApiPortFile.urlFor("::1", 5555), "IPv6 要加方括号")
    }
}

/**
 * 端口默认值的行为（`AppConfig.parsePort`）。
 *
 * 用户的要求是"**先把它初始化到 0（随机端口）**；之后用户指定了就用用户的，
 * 失败了回到随机端口"。所以这里钉三件事：没给 = 0（不是 8080）、给 0 = 0、
 * 给了正经端口就用它、给了垃圾**也不许崩**（0 兜底）。
 *
 * 这里**直接调生产的那个函数**（`internal` 可见），不是照抄一遍判据 ——
 * 抄一遍就只是"测试自己跟自己一致"，改坏了生产代码它照样绿。
 */
class ConfigPortTest {

    private fun portOf(raw: String?): Int = AppConfig.parsePort(raw)

    @Test
    fun `没给端口时默认 0 让操作系统挑`() {
        assertEquals(0, portOf(null), "出厂默认必须是'自动'，不能再写死 8080")
        assertEquals(0, portOf(""))
        assertEquals(0, portOf("   "))
        assertEquals(0, portOf("0"), "显式要随机也是 0")
    }

    @Test
    fun `显式给的端口照用（这是'用户指定了就用用户的'）`() {
        assertEquals(8280, portOf("8280"))
        assertEquals(8280, portOf(" 8280 "), "两头空格是手抄配置的常态")
        assertEquals(65535, portOf("65535"))
    }

    @Test
    fun `写错的端口不许把后端崩在解析阶段`() {
        listOf("abc", "http://127.0.0.1:8080", "-1", "70000", "80.80").forEach { bad ->
            assertEquals(0, portOf(bad), "「$bad」应当回落成自动，而不是抛异常")
        }
    }

    @Test
    fun `默认配置里端口就是 0（出厂不再写死 8080）`() {
        // 直接把"默认"这件事钉在数据类上：改回 8080 会在这里红
        val cfg = AppConfig(
            host = "127.0.0.1",
            port = AppConfig.parsePort(null),
            jdbcUrl = "jdbc:mysql://127.0.0.1:3307/comfy_hub",
            dbUser = "u",
            dbPassword = "p",
            storageDir = java.nio.file.Files.createTempDirectory("comfyhub-cfg").toRealPath(),
            maxUploadBytes = 1,
        )
        assertEquals(0, cfg.port)
        assertTrue(cfg.loopbackOnly)
    }
}
