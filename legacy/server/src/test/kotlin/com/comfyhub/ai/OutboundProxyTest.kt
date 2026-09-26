package com.comfyhub.ai

import java.net.InetSocketAddress
import java.net.Proxy
import java.net.ProxySelector
import java.net.URI
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * 出网代理的判据（**不碰网络**：只是选择器与解析的纯逻辑）。
 *
 * 这段代码是"用户报的 civitai_search 反复超时"的修复点：JVM 默认一律直连，
 * 得先把本机的代理读出来；但**读了代理之后绝不能把本机 / 内网也代理掉** ——
 * 实测打开 `useSystemProxies` 后 `127.0.0.1:8188`、`192.168.*` 全被判成走代理，
 * 那会当场打死本机 ComfyUI。所以两边都要有用例钉着。
 */
class OutboundProxyTest {

    private val deadProxy = Proxy(Proxy.Type.HTTP, InetSocketAddress.createUnresolved("127.0.0.1", 7890))

    /** URI 里的 IPv6 字面量**必须带方括号**，否则 `URI.host` 是 null（测试自己踩过这个坑）。 */
    private fun uriFor(host: String): URI {
        val h = if (host.contains(':') && !host.startsWith("[")) "[$host]" else host
        return URI("https://$h/x")
    }

    private fun select(host: String, delegate: ProxySelector = FixedProxySelector(deadProxy)) =
        GuardedProxySelector(delegate).select(uriFor(host)).single()

    // --- 本机 / 内网：永不代理（这条错了就是 ComfyUI 失联） -------------------

    @Test
    fun `本机与内网地址永远直连`() {
        val locals = listOf(
            "localhost", "127.0.0.1", "127.1.2.3", "::1", "[::1]",
            "10.0.0.5", "192.168.1.50", "172.16.0.1", "172.31.255.254",
            "169.254.10.10", "0.0.0.0",
            "fc00::1", "fd12:3456::1", "fe80::1",
            "comfy.local", "box.lan", "nas.internal", "printer.home.arpa", "dev.localhost",
        )
        for (h in locals) {
            assertTrue(OutboundProxy.isLocalHost(h), "$h 应当算本机 / 内网")
            assertEquals(Proxy.NO_PROXY, select(h), "$h 必须直连（代理会把它吃掉）")
        }
    }

    @Test
    fun `公网地址走代理设置`() {
        val publics = listOf(
            "civitai.com", "civitai.red", "api.commandcode.ai", "api.deepseek.com",
            "8.8.8.8", "172.32.0.1", // 172.32 已经出了私有段
            "2001:4860:4860::8888",
        )
        for (h in publics) {
            assertFalse(OutboundProxy.isLocalHost(h), "$h 是公网地址")
            assertEquals(deadProxy, select(h), "$h 应当按本机代理设置走")
        }
    }

    /** 没有主机名（相对 URI 之类）时按本机处理：宁可直连，也别把请求送去代理。 */
    @Test
    fun `没有主机名时按直连处理`() {
        assertTrue(OutboundProxy.isLocalHost(null))
        assertTrue(OutboundProxy.isLocalHost(""))
        assertTrue(OutboundProxy.isLocalHost("   "))
    }

    /** 域名解析不该被触发：一看就不是 IP 字面量的名字直接判公网，不做 DNS。 */
    @Test
    fun `不像 IP 字面量的名字不做 DNS`() {
        assertFalse(OutboundProxy.isLocalHost("definitely-not-a-real-host.example"))
    }

    // --- SOCKS：java.net.http 不支持，必须滤掉 ---------------------------------

    @Test
    fun `SOCKS 代理被滤掉 而不是发出去必然失败`() {
        val socks = Proxy(Proxy.Type.SOCKS, InetSocketAddress.createUnresolved("127.0.0.1", 1080))
        val chosen = GuardedProxySelector(FixedProxySelector(socks))
            .select(URI("https://civitai.com/x")).single()
        assertEquals(Proxy.NO_PROXY, chosen)
    }

    // --- 环境变量优先级 / 写法 ------------------------------------------------

    @Test
    fun `COMFYHUB_HTTPS_PROXY 优先于通用变量`() {
        val env = mapOf(
            OutboundProxy.ENV_PROXY to "http://10.1.1.1:3128",
            "HTTPS_PROXY" to "http://10.2.2.2:3128",
        )
        val configured = OutboundProxy.readConfigured(env::get)
        assertTrue(configured is OutboundProxy.Configured.Via, "应当解析出代理：$configured")
        val addr = (configured as OutboundProxy.Configured.Via).proxy.address() as InetSocketAddress
        assertEquals("10.1.1.1", addr.hostString)
        assertEquals(3128, addr.port)
    }

    @Test
    fun `通用变量依次兜底 大小写都认`() {
        assertTrue(OutboundProxy.readConfigured(mapOf("https_proxy" to "1.2.3.4:8080")::get)
            is OutboundProxy.Configured.Via)
        assertTrue(OutboundProxy.readConfigured(mapOf("HTTP_PROXY" to "1.2.3.4")::get)
            is OutboundProxy.Configured.Via)
    }

    @Test
    fun `direct 等写法表示明确要求直连 用来盖掉系统代理`() {
        for (word in listOf("direct", "DIRECT", "none", "off", "-")) {
            assertEquals(
                OutboundProxy.Configured.ForceDirect,
                OutboundProxy.readConfigured(mapOf(OutboundProxy.ENV_PROXY to word)::get),
                "$word 应当表示强制直连",
            )
        }
        // 强制直连时**公网也不走代理**（这是给"系统代理坏了"留的逃生门）
        val forced = OutboundProxy.build(mapOf(OutboundProxy.ENV_PROXY to "direct")::get)
        assertEquals(Proxy.NO_PROXY, forced.select(URI("https://civitai.com/x")).single())
    }

    @Test
    fun `没配任何变量时不硬造代理`() {
        assertEquals(OutboundProxy.Configured.NotSet, OutboundProxy.readConfigured { null })
        // 写坏了（解析不出主机）也按"没配"处理，回落系统代理，而不是把请求送到一个瞎猜的地址
        assertEquals(OutboundProxy.Configured.NotSet, OutboundProxy.parseProxy("这不是一个地址"))
    }

    @Test
    fun `host 不带端口时按方案给默认端口`() {
        val via = OutboundProxy.parseProxy("http://proxy.example") as OutboundProxy.Configured.Via
        assertEquals(80, (via.proxy.address() as InetSocketAddress).port)
        val tls = OutboundProxy.parseProxy("https://proxy.example") as OutboundProxy.Configured.Via
        assertEquals(443, (tls.proxy.address() as InetSocketAddress).port)
    }

    /** 错误信息里那半句话要能看懂：直连 / 代理各是什么样。 */
    @Test
    fun `describe 给出人话`() {
        val label = OutboundProxy.routeLabel(deadProxy)
        assertTrue(label.contains("127.0.0.1:7890"), label)
        assertTrue(label.contains("代理"), label)
        assertEquals("直连（DIRECT）", OutboundProxy.routeLabel(Proxy.NO_PROXY))
        assertEquals("直连（DIRECT）", OutboundProxy.routeLabel(null))
    }
}
