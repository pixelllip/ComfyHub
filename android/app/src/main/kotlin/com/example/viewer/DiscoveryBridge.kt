package com.example.viewer

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.SocketException

/**
 * 局域网**桌面发现**（与 Rust `mobile::discovery` 配套）。
 *
 * 桌面每 3 秒往各网卡广播地址发一条 100 字节 JSON：
 * `{"service":"comfyhub","v":1,"port":4038}`。
 * 手机绑 UDP 4039 收包，用**源地址**拼 `http://<source>:<port>`。
 *
 * 三条边界：
 *  · 只覆盖**同一广播域**；换网段（或 AP 隔离）收不到，由 Tailscale / 手填地址兜底；
 *  · 广播**不是凭据**：这里只发现地址，能不能读数据仍由配对令牌决定；
 *  · 收包在 IO 线程，`EventSink` 必须回主线程（否则与 2026-09-24 那个
 *    `Methods marked with @UiThread` 崩溃同类）。
 */
class DiscoveryBridge(private val context: Context) :
    MethodChannel.MethodCallHandler,
    EventChannel.StreamHandler {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val main = Handler(Looper.getMainLooper())

    private var methodChannel: MethodChannel? = null
    private var eventChannel: EventChannel? = null
    private var sink: EventChannel.EventSink? = null
    private var job: Job? = null
    private var socket: DatagramSocket? = null
    private var multicast: WifiManager.MulticastLock? = null

    /** host:port → 上次上报的 elapsedRealtime，避免每 3 秒刷同一条。 */
    private val lastSeen = HashMap<String, Long>()

    fun attach(messenger: BinaryMessenger) {
        methodChannel = MethodChannel(messenger, CHANNEL).also { it.setMethodCallHandler(this) }
        eventChannel = EventChannel(messenger, EVENT_CHANNEL).also { it.setStreamHandler(this) }
    }

    fun detach() {
        stop()
        methodChannel?.setMethodCallHandler(null)
        eventChannel?.setStreamHandler(null)
        methodChannel = null
        eventChannel = null
        sink = null
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "start" -> {
                start()
                result.success(true)
            }
            "stop" -> {
                stop()
                result.success(true)
            }
            else -> result.notImplemented()
        }
    }

    override fun onListen(arguments: Any?, events: EventChannel.EventSink) {
        sink = events
        start()
    }

    override fun onCancel(arguments: Any?) {
        stop()
        sink = null
    }

    @Synchronized
    private fun start() {
        if (job != null) return
        val lock = try {
            val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            wifi?.createMulticastLock("comfyhub-discovery")?.apply {
                setReferenceCounted(false)
                acquire()
            }
        } catch (_: Throwable) {
            // 没有 CHANGE_WIFI_MULTICAST_STATE 权限时仍可能收到广播；别因此让发现整体不可用
            null
        }
        multicast = lock
        job = scope.launch {
            val udp = try {
                DatagramSocket(null).apply {
                    reuseAddress = true
                    broadcast = true
                    bind(InetSocketAddress(PORT))
                }
            } catch (e: Throwable) {
                postError("绑定 UDP $PORT 失败：${e.message}")
                job = null
                return@launch
            }
            socket = udp
            val buffer = ByteArray(2048)
            while (isActive) {
                val packet = DatagramPacket(buffer, buffer.size)
                try {
                    udp.receive(packet)
                } catch (_: SocketException) {
                    break
                } catch (_: Throwable) {
                    continue
                }
                handle(packet)
            }
        }
    }

    @Synchronized
    private fun stop() {
        job?.cancel()
        job = null
        socket?.close()
        socket = null
        multicast?.release()
        multicast = null
        lastSeen.clear()
    }

    private fun handle(packet: DatagramPacket) {
        val text = String(packet.data, 0, packet.length, Charsets.UTF_8)
        val json = try {
            JSONObject(text)
        } catch (_: Throwable) {
            return
        }
        if (json.optString("service") != "comfyhub") return
        val port = json.optInt("port", 0)
        if (port !in 1..65535) return
        val host = packet.address?.hostAddress?.trim().orEmpty()
        if (host.isEmpty() || host.startsWith("127.") || host == "::1") return
        val key = "$host:$port"
        val now = SystemClock.elapsedRealtime()
        val prev = lastSeen[key]
        if (prev != null && now - prev < DEDUPE_MS) return
        lastSeen[key] = now
        main.post {
            sink?.success(
                mapOf(
                    "url" to "http://$host:$port",
                    "host" to host,
                    "port" to port,
                )
            )
        }
    }

    private fun postError(message: String) {
        main.post { sink?.error("DISCOVERY", message, null) }
    }

    companion object {
        const val CHANNEL = "comfyhub/discovery"
        const val EVENT_CHANNEL = "comfyhub/discovery/events"

        /** 与 Rust `mobile::discovery::DISCOVERY_PORT` 同值（改一处要同时改另一处）。 */
        const val PORT = 4039

        private const val DEDUPE_MS = 15_000L
    }
}
