package com.comfyhub.harness.net

import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.net.HttpURLConnection
import java.net.URL
import java.nio.charset.StandardCharsets

/**
 * [HttpTransport] 的生产实现：`HttpURLConnection`（JDK / Android 自带，**不引第三方 HTTP 客户端**）。
 *
 * 为什么不用 OkHttp：手机 harness 的 HTTP 需求只有"GET 一个 JSON / POST 一个 JSON / 读一行行 SSE"，
 * 而 `HttpURLConnection` 已经够用，还少一个依赖（依赖越少，Rust 之后的重写面越小）。
 * 桌面侧用 `java.net.http.HttpClient`，两边各自用平台现成的即可 —— 管道不需要统一。
 *
 * 三条与安全有关的实现细节：
 *  1. **不跟随重定向**（`instanceFollowRedirects = false`）：防止 `Authorization` 被带到别的域名
 *     （桌面 `AiUpstream` 是同一条纪律，`Redirect.NEVER`）；
 *  2. **不记录任何请求头**（尤其是密钥）：异常信息里只出现 URL 与状态码；
 *  3. 读流一律 **UTF-8**，SSE **按行**读（`BufferedReader.readLine` 天然处理 `\r\n`）。
 */
class UrlConnectionTransport(
    private val connectTimeoutMs: Int = 15_000,
    private val readTimeoutMs: Int = 120_000,
) : HttpTransport {

    override fun get(url: String, headers: Map<String, String>): HttpReply {
        val conn = open(url, "GET", headers)
        return read(conn)
    }

    override fun postJson(url: String, body: String, headers: Map<String, String>): HttpReply {
        val conn = open(url, "POST", headers + mapOf("Content-Type" to "application/json"))
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toByteArray(StandardCharsets.UTF_8)) }
        return read(conn)
    }

    override fun postJsonStreaming(
        url: String,
        body: String,
        headers: Map<String, String>,
        onLine: (String) -> Unit,
    ): Int {
        val conn = open(url, "POST", headers + mapOf(
            "Content-Type" to "application/json",
            "Accept" to "text/event-stream",
        ))
        conn.doOutput = true
        conn.outputStream.use { it.write(body.toByteArray(StandardCharsets.UTF_8)) }

        val status = conn.responseCode
        if (status !in 200..299) {
            // ⚠ 非 2xx **绝不喂给事件流解析器**：错误页被当成 SSE 只会在上层变成莫名其妙的空回复
            val detail = runCatching { conn.errorStream?.bufferedReader()?.readText().orEmpty() }.getOrDefault("")
            conn.disconnect()
            throw HttpFailure(status, "上游返回 HTTP $status：${detail.take(300)}")
        }
        BufferedReader(InputStreamReader(conn.inputStream, StandardCharsets.UTF_8)).use { reader ->
            while (true) {
                val line = reader.readLine() ?: break
                onLine(line)
            }
        }
        conn.disconnect()
        return status
    }

    private fun open(url: String, method: String, headers: Map<String, String>): HttpURLConnection {
        val conn = URL(url).openConnection() as HttpURLConnection
        conn.requestMethod = method
        conn.connectTimeout = connectTimeoutMs
        conn.readTimeout = readTimeoutMs
        conn.instanceFollowRedirects = false
        headers.forEach { (k, v) -> conn.setRequestProperty(k, v) }
        return conn
    }

    private fun read(conn: HttpURLConnection): HttpReply {
        val status = conn.responseCode
        val stream = if (status in 200..299) conn.inputStream else conn.errorStream
        val text = stream?.use { s -> InputStreamReader(s, StandardCharsets.UTF_8).readText() }.orEmpty()
        conn.disconnect()
        return HttpReply(status, text)
    }

    /** 给"要把 JSON 写出去"的调用方用（避免它们各自再拼字符集）。 */
    companion object {
        fun writerOf(conn: HttpURLConnection) = OutputStreamWriter(conn.outputStream, StandardCharsets.UTF_8)
    }
}
