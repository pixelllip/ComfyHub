package com.comfyhub.harness.net

/**
 * 出网的最小抽象：只暴露 harness 真正需要的两件事（**取 JSON** 与 **流式读一行行**）。
 *
 * 为什么要接口：手机侧的 I/O 全部走注入（`docs/mobile-harness-design.md` §2.1 的"为将来 KMP 留门"），
 * 于是协议层 / 工具层 / kit 客户端都能在**宿主 JVM 的单元测试**里跑，
 * 不用模拟器、也不出网（用例给一个假 transport 就行）。
 *
 * ⚠ 这里**只做管道**：不解析、不重试、不记录任何请求头 ——
 * 「绝不记录 Authorization / x-api-key」这条纪律由调用方（上游客户端）负责。
 */
interface HttpTransport {

    fun get(url: String, headers: Map<String, String> = emptyMap()): HttpReply

    fun postJson(url: String, body: String, headers: Map<String, String> = emptyMap()): HttpReply

    /**
     * `POST` 一个 JSON 体，并**按行**把响应体喂给 [onLine]（SSE 用）。
     *
     * @return HTTP 状态码；非 2xx 时不调用 [onLine]（**绝不能把错误页当事件流解析**）。
     */
    fun postJsonStreaming(
        url: String,
        body: String,
        headers: Map<String, String> = emptyMap(),
        onLine: (String) -> Unit,
    ): Int
}

/** 一次非流式响应。 */
data class HttpReply(val status: Int, val body: String) {
    val ok: Boolean get() = status in 200..299

    /** 2xx 才算成功；否则把状态码与截断后的正文一起报出来（调用方归到稳定错误码）。 */
    fun requireOk(): HttpReply =
        if (ok) this else throw HttpFailure(status, "HTTP $status：${body.take(300)}")
}

/** 网络层失败（区别于"上游返回了错误码"）。 */
class HttpFailure(val status: Int?, message: String) : RuntimeException(message)
