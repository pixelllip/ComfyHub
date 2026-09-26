package com.comfyhub.ai

import java.io.IOException
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Proxy
import java.net.ProxySelector
import java.net.SocketAddress
import java.net.URI
import org.slf4j.LoggerFactory

/**
 * 出网走不走代理 —— **JVM 默认不看 Windows 的系统代理**（2026-09-20）。
 *
 * ## 用户报的现场
 *
 * "civitai_search 方法反复超时"。根因不在 civitai，也不在解析：
 * 本机 `civitai.com:443` **直连不通**（要到 `127.0.0.1:7890` 那个代理才通，系统代理里
 * 就写着它），而 `java.net.http.HttpClient` **默认一律 DIRECT** —— Java 只有在显式给了
 * `-Djava.net.useSystemProxies=true` / `-Dhttps.proxyHost=...` 时才认代理。
 * 实测同一个 JVM、同一个 URL：
 *
 * | 启动参数 | 选到的路径 | 结果 |
 * | --- | --- | --- |
 * | 没有 | `[DIRECT]` | `HttpConnectTimeoutException`（用户看到的"反复超时"） |
 * | `-Djava.net.useSystemProxies=true` | `[HTTP @ 127.0.0.1:7890]` | 200，860ms |
 *
 * ## 为什么不能只是"把那个 flag 打开"
 *
 * 打开 flag 之后 Java 会**连内网也塞给代理**（实测 `127.0.0.1:8188`、`192.168.*`、
 * `10.*` 全被判成走代理，只有字面量 `localhost` 例外）—— 那会当场打死本机 ComfyUI 与
 * 本机模型网关这类"127.0.0.1"的调用。所以这里套一层 [GuardedProxySelector]：
 * **公网地址按本机设置走代理，本机 / 内网地址永远直连**。
 *
 * ## 也不能指望"代理不行就退回直连"
 *
 * `ProxySelector.select` 返回的是列表，但 `java.net.http` **不会**在列表里依次重试
 * （实测：给 `[死代理, DIRECT]` 时它 38ms 就抛 `ConnectException`，没有退到第二个）。
 * 所以路径必须一开始就选对；选错了只能在错误信息里说清楚（见 [describe]）。
 *
 * ## 优先级
 *
 *  1. 环境变量 `COMFYHUB_HTTPS_PROXY` → `HTTPS_PROXY` / `https_proxy` / `HTTP_PROXY` / `http_proxy`
 *     （写成 `direct` / `none` / `off` 表示**明确要求直连**，用来盖掉系统代理）；
 *  2. 系统代理（Windows 的 WinINET 设置，靠启动参数 `-Djava.net.useSystemProxies=true`
 *     才会被 Java 读取，见 `server/build.gradle.kts` 的 `applicationDefaultJvmArgs`）；
 *  3. 都没有 → DIRECT。
 *
 * 本机 / 内网地址（[isLocalHost]）**永远**走 1/2 之上的旁路：写死 DIRECT。
 */
object OutboundProxy {
    private val log = LoggerFactory.getLogger(OutboundProxy::class.java)

    /** 本项目的显式优先级最高的代理变量。 */
    const val ENV_PROXY = "COMFYHUB_HTTPS_PROXY"

    /** 通用的几个写法（大小写都认，很多人习惯小写）。 */
    val ENV_FALLBACKS = listOf("HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy")

    /** 显式要求"别走代理"的写法。 */
    val DIRECT_WORDS = setOf("direct", "none", "off", "-", "no")

    private val selector: ProxySelector by lazy { build(javaSystemEnv) }

    /** 所有出网客户端都该用的那个选择器。 */
    fun selector(): ProxySelector = selector

    /**
     * 这次请求会怎么走 —— **只用来写进错误信息**。
     *
     * 用户看不到我们的日志，但看得到工具报错；"直连"与"走 127.0.0.1:7890"这两个答案
     * 能让人一眼判断是代理的事还是对方的事。返回的是给人看的中文短句。
     */
    fun describe(host: String?): String {
        val uri = runCatching { URI("https://${host ?: "?"}/") }.getOrNull() ?: return "未知"
        val chosen = runCatching { selector.select(uri).firstOrNull() }.getOrNull()
        return routeLabel(chosen)
    }

    /** 给人看的路径名（[describe] 与日志共用）。 */
    internal fun routeLabel(proxy: Proxy?): String = when {
        proxy == null || proxy.type() == Proxy.Type.DIRECT -> "直连（DIRECT）"
        proxy.address() is InetSocketAddress -> {
            val a = proxy.address() as InetSocketAddress
            "${proxy.type()} 代理 ${a.hostString}:${a.port}"
        }
        else -> proxy.toString()
    }

    // -----------------------------------------------------------------------
    //  本机 / 内网判定：公网才值得代理
    // -----------------------------------------------------------------------

    /**
     * 这个主机名该不该**永不代理**。
     *
     * 判据刻意宽松（宁可直连）：字面量 loopback / 私有网段 / 链路本地 / ULA，
     * 以及 `localhost`、`*.local`、`*.lan`、`*.internal` 这些一看就是本机内网的名字。
     * **绝不为它做 DNS**：只有"看起来就是 IP 字面量"的才去解析。
     * 解析不出来的也按本机处理 —— 代理本来就只该用来解决"公网连不上"。
     */
    fun isLocalHost(host: String?): Boolean {
        val h = host?.trim()?.lowercase()?.removeSurrounding("[", "]") ?: return true
        if (h.isEmpty()) return true
        if (h == "localhost" || h.endsWith(".localhost")) return true
        if (LOCAL_SUFFIXES.any { h.endsWith(it) }) return true
        if (!looksLikeIpLiteral(h)) return false
        val addr = runCatching { InetAddress.getByName(h) }.getOrNull() ?: return true
        return addr.isLoopbackAddress ||
            addr.isSiteLocalAddress ||
            addr.isLinkLocalAddress ||
            addr.isAnyLocalAddress ||
            addr.isMulticastAddress ||
            isUniqueLocalV6(addr)
    }

    /** `fc00::/7`（IPv6 的"内网"）。 */
    private fun isUniqueLocalV6(addr: InetAddress): Boolean {
        val b = addr.address
        return b.size == 16 && (b[0].toInt() and 0xfe) == 0xfc
    }

    private fun looksLikeIpLiteral(h: String): Boolean =
        h.contains(':') || h.all { it.isDigit() || it == '.' }

    private val LOCAL_SUFFIXES = listOf(
        ".local", ".lan", ".internal", ".intranet", ".home", ".home.arpa", ".localdomain",
    )

    // -----------------------------------------------------------------------
    //  解析与装配
    // -----------------------------------------------------------------------

    /** 三态：没配（看系统）/ 明确直连 / 走某个代理。 */
    internal sealed interface Configured {
        data object NotSet : Configured
        data object ForceDirect : Configured
        data class Via(val proxy: Proxy) : Configured
    }

    private val javaSystemEnv: (String) -> String? = { System.getenv(it) }

    /**
     * 按优先级读出配置。**测试直接调它**（注入 env），不碰真实环境变量。
     */
    internal fun readConfigured(env: (String) -> String?): Configured {
        val raw = env(ENV_PROXY)?.takeIf { it.isNotBlank() }
            ?: ENV_FALLBACKS.firstNotNullOfOrNull { env(it)?.takeIf { it.isNotBlank() } }
            ?: return Configured.NotSet
        return parseProxy(raw)
    }

    /**
     * 解析一个代理写法：`http://host:port`、`host:port`、`host`，或 `direct`/`none`/`off`。
     *
     * 只造 **HTTP** 代理：`java.net.http` 不支持 SOCKS（给了会直接失败），
     * 而 Clash 这类混合端口的系统代理本来就以 HTTP 形式报给 WinINET。
     * 写不清楚（没法解析出主机）时按"没配"处理，回落系统代理。
     */
    internal fun parseProxy(raw: String): Configured {
        val v = raw.trim()
        if (v.lowercase() in DIRECT_WORDS) return Configured.ForceDirect
        val uri = runCatching {
            if (v.contains("://")) URI(v) else URI("http://$v")
        }.getOrNull() ?: return Configured.NotSet
        val host = uri.host?.takeIf { it.isNotBlank() } ?: return Configured.NotSet
        val port = uri.port.takeIf { it > 0 } ?: if (uri.scheme.equals("https", true)) 443 else 80
        return Configured.Via(Proxy(Proxy.Type.HTTP, InetSocketAddress.createUnresolved(host, port)))
    }

    /** 装出一个选择器：显式配置 > 系统代理，外面统一套本机旁路。 */
    internal fun build(env: (String) -> String?): ProxySelector {
        val configured = readConfigured(env)
        val base: ProxySelector = when (configured) {
            is Configured.Via -> FixedProxySelector(configured.proxy)
            Configured.ForceDirect -> FixedProxySelector(null)
            Configured.NotSet -> ProxySelector.getDefault() ?: FixedProxySelector(null)
        }
        val guarded = GuardedProxySelector(base)
        log.info(
            "出网代理: {}（来源={}；本机/内网地址永远直连）",
            routeLabel(guarded.select(URI("https://example.com/")).firstOrNull()),
            when (configured) {
                is Configured.Via -> ENV_PROXY
                Configured.ForceDirect -> "环境变量=direct"
                Configured.NotSet -> "系统代理（WinINET，需 -Djava.net.useSystemProxies=true）"
            },
        )
        return guarded
    }
}

/**
 * 本机 / 内网地址写死 DIRECT，其余交给 [delegate]。
 *
 * 这层不是"锦上添花"：打开系统代理之后 Java 会把 `127.0.0.1:8188` 也交给代理
 * （实测，见 [OutboundProxy] 的类注释），本机 ComfyUI 会当场失联。
 */
internal class GuardedProxySelector(private val delegate: ProxySelector) : ProxySelector() {
    override fun select(uri: URI?): MutableList<Proxy> {
        if (OutboundProxy.isLocalHost(uri?.host)) return mutableListOf(Proxy.NO_PROXY)
        val picked = runCatching { delegate.select(uri) }.getOrNull().orEmpty()
        // java.net.http 只支持 HTTP 代理：把 SOCKS / 其它类型滤掉，
        // 剩下的空列表就当直连（真有问题会体现在错误信息里的"直连"上）。
        val http = picked.filter { it.type() == Proxy.Type.HTTP }
        return if (http.isEmpty()) mutableListOf(Proxy.NO_PROXY) else http.toMutableList()
    }

    override fun connectFailed(uri: URI?, sa: SocketAddress?, ioe: IOException?) {
        runCatching { delegate.connectFailed(uri, sa, ioe) }
    }
}

/** 固定一个结果的选择器（显式配置 / 没有系统代理时的兜底）。`null` = 永远 DIRECT。 */
internal class FixedProxySelector(private val proxy: Proxy?) : ProxySelector() {
    override fun select(uri: URI?): MutableList<Proxy> =
        mutableListOf(proxy ?: Proxy.NO_PROXY)

    override fun connectFailed(uri: URI?, sa: SocketAddress?, ioe: IOException?) = Unit
}
