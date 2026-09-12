package com.comfyhub

import io.ktor.http.ContentDisposition
import io.ktor.http.ContentType
import io.ktor.http.HttpHeaders
import io.ktor.http.HttpStatusCode
import io.ktor.http.Parameters
import io.ktor.server.application.ApplicationCall
import io.ktor.server.response.header
import io.ktor.server.response.respond
import java.nio.charset.StandardCharsets

/** 统一的错误响应 */
suspend fun ApplicationCall.respondError(
    status: HttpStatusCode,
    message: String,
    detail: String? = null,
) = respond(status, ApiError(error = message, detail = detail))

suspend fun ApplicationCall.respondBadRequest(message: String, detail: String? = null) =
    respondError(HttpStatusCode.BadRequest, message, detail)

suspend fun ApplicationCall.respondNotFound(message: String = "资源不存在") =
    respondError(HttpStatusCode.NotFound, message)

suspend fun ApplicationCall.requireId(name: String = "id"): Long? =
    parameters[name]?.toLongOrNull()

// ---------------------------------------------------------------------------
//  查询参数读取
// ---------------------------------------------------------------------------

fun Parameters.str(name: String): String? = this[name]?.takeIf { it.isNotBlank() }

fun Parameters.int(name: String, default: Int): Int = this[name]?.toIntOrNull() ?: default

fun Parameters.long(name: String): Long? = this[name]?.toLongOrNull()

fun Parameters.bool(name: String): Boolean? = when (this[name]?.lowercase()) {
    "1", "true", "yes" -> true
    "0", "false", "no" -> false
    else -> null
}

// ---------------------------------------------------------------------------
//  文件下载响应头
// ---------------------------------------------------------------------------

fun ApplicationCall.setInlineFileHeaders(fileName: String, contentType: ContentType) {
    response.header(HttpHeaders.ContentType, contentType.toString())
    response.header(HttpHeaders.AcceptRanges, "bytes")
    response.header(HttpHeaders.CacheControl, "public, max-age=86400")
    val ascii = fileName.map { if (it.code in 32..126 && it != '"' && it != '\\') it else '_' }.joinToString("")
    val encoded = java.net.URLEncoder.encode(fileName, StandardCharsets.UTF_8).replace("+", "%20")
    response.header(
        HttpHeaders.ContentDisposition,
        "inline; filename=\"$ascii\"; filename*=UTF-8''$encoded"
    )
}

fun contentTypeFor(mime: String?): ContentType =
    runCatching { ContentType.parse(mime ?: "application/octet-stream") }
        .getOrElse { ContentType.Application.OctetStream }

/** 供日志使用：解析 ContentDisposition 里的文件名 */
fun contentDispositionInline(name: String): ContentDisposition =
    ContentDisposition.Inline.withParameter(ContentDisposition.Parameters.FileName, name)
