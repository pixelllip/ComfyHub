package com.comfyhub.ai

/**
 * 严格文件类型识别（AIH-027 / RSK-008）。
 *
 * **为什么不复用 `MediaFiles.kindOf()`**：那个函数的兜底是"未知即 IMAGE"，
 * 用在画廊里只是显示不准，用在模型输入上就是**把未知文件当图片发给上游**。
 * 这里反过来：**签名优先，认不出来就是 UNKNOWN，UNKNOWN 一律阻断**。
 *
 * 扩展名和声明 MIME 只作为辅助（用于纯文本这类没有魔数的类型），
 * 绝不单独作为放行依据。
 */
enum class FileKind(val modality: Modality?) {
    IMAGE(Modality.IMAGE),
    VIDEO(Modality.VIDEO),
    AUDIO(Modality.AUDIO),
    DOCUMENT(Modality.DOCUMENT),
    TEXT(Modality.TEXT),
    UNKNOWN(null),
}

data class DetectedFile(
    val kind: FileKind,
    /** 探测出的 MIME；UNKNOWN 时为 null */
    val mimeType: String?,
    /** 判断依据：signature / text-sniff / none（便于审计与排错） */
    val basis: String,
) {
    val modality: Modality? get() = kind.modality
    val blocked: Boolean get() = kind == FileKind.UNKNOWN
}

object FileKindDetector {

    /** 纯文本扩展名白名单：这些类型没有魔数，只能靠内容嗅探 + 扩展名共同确认。 */
    private val TEXT_EXT = setOf("txt", "md", "markdown", "csv", "tsv", "json", "yaml", "yml", "log")

    /**
     * @param head        文件开头若干字节（建议 ≥ 64B；读不到这么多就传实际读到的）
     * @param fileName    原始文件名（取扩展名用）
     * @param declaredMime 上游声明的 MIME，可为 null
     */
    fun detect(head: ByteArray, fileName: String, declaredMime: String? = null): DetectedFile {
        val ext = fileName.substringAfterLast('.', "").lowercase().take(12)

        // 1) 魔数优先
        signature(head, ext)?.let { return it }

        // 2) ISO BMFF（mp4/mov/avif/heic…）：ftyp 盒子
        ftypMime(head)?.let { mime ->
            val kind = if (mime.startsWith("video/")) FileKind.VIDEO else FileKind.IMAGE
            return DetectedFile(kind, mime, "signature:ftyp")
        }

        // 3) 纯文本：内容全是可打印 UTF-8 且扩展名在白名单里才算数
        if (head.isNotEmpty() && ext in TEXT_EXT && looksLikeText(head)) {
            return DetectedFile(FileKind.TEXT, textMime(ext), "text-sniff")
        }

        // 4) 声明 MIME 是已知的文本类，且扩展名也在白名单 → 仍然只当文本
        if (ext in TEXT_EXT && declaredMime?.startsWith("text/") == true && looksLikeText(head)) {
            return DetectedFile(FileKind.TEXT, textMime(ext), "text-sniff")
        }

        // 5) 认不出来 —— 不认识就是不认识，不做任何乐观回退
        return DetectedFile(FileKind.UNKNOWN, null, "none")
    }

    // -----------------------------------------------------------------------

    private fun signature(b: ByteArray, ext: String): DetectedFile? {
        fun at(i: Int, vararg bytes: Int): Boolean {
            if (b.size < i + bytes.size) return false
            return bytes.withIndex().all { (k, v) -> (b[i + k].toInt() and 0xFF) == v }
        }
        fun ascii(i: Int, s: String): Boolean =
            at(i, *s.map { it.code }.toIntArray())

        return when {
            at(0, 0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A) ->
                DetectedFile(FileKind.IMAGE, "image/png", "signature")
            at(0, 0xFF, 0xD8, 0xFF) ->
                DetectedFile(FileKind.IMAGE, "image/jpeg", "signature")
            ascii(0, "GIF87a") || ascii(0, "GIF89a") ->
                DetectedFile(FileKind.IMAGE, "image/gif", "signature")
            ascii(0, "RIFF") && ascii(8, "WEBP") ->
                DetectedFile(FileKind.IMAGE, "image/webp", "signature")
            at(0, 0x42, 0x4D) ->
                DetectedFile(FileKind.IMAGE, "image/bmp", "signature")
            at(0, 0x49, 0x49, 0x2A, 0x00) || at(0, 0x4D, 0x4D, 0x00, 0x2A) ->
                DetectedFile(FileKind.IMAGE, "image/tiff", "signature")

            // 音视频容器
            ascii(0, "RIFF") && ascii(8, "WAVE") ->
                DetectedFile(FileKind.AUDIO, "audio/wav", "signature")
            at(0, 0x1A, 0x45, 0xDF, 0xA3) ->
                DetectedFile(FileKind.VIDEO, "video/webm", "signature:ebml")
            ascii(0, "OggS") ->
                DetectedFile(FileKind.AUDIO, "audio/ogg", "signature")
            ascii(0, "fLaC") ->
                DetectedFile(FileKind.AUDIO, "audio/flac", "signature")
            ascii(0, "ID3") || at(0, 0xFF, 0xFB) || at(0, 0xFF, 0xF3) || at(0, 0xFF, 0xF2) ->
                DetectedFile(FileKind.AUDIO, "audio/mpeg", "signature")

            // 文档
            ascii(0, "%PDF-") ->
                DetectedFile(FileKind.DOCUMENT, "application/pdf", "signature")
            at(0, 0x50, 0x4B, 0x03, 0x04) ->
                DetectedFile(FileKind.DOCUMENT, ooxmlMime(ext), "signature:zip")

            else -> null
        }
    }

    /** ISO BMFF：`....ftyp<brand>`，用品牌的 MIME 前缀区分图片 / 视频。 */
    private fun ftypMime(b: ByteArray): String? {
        if (b.size < 12) return null
        if (!(b[4] == 'f'.code.toByte() && b[5] == 't'.code.toByte() &&
                b[6] == 'y'.code.toByte() && b[7] == 'p'.code.toByte())
        ) {
            return null
        }
        val brand = String(b, 8, 4, Charsets.US_ASCII).trim().lowercase()
        return when (brand) {
            "avif", "avis" -> "image/avif"
            "heic", "heix", "hevc", "mif1" -> "image/heic"
            "qt  " -> "video/quicktime"
            "isom", "iso2", "mp41", "mp42", "avc1", "dash", "m4v ", "msnv" -> "video/mp4"
            else -> null
        }
    }

    private fun ooxmlMime(ext: String): String = when (ext) {
        "docx" -> "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        "xlsx" -> "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        "pptx" -> "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        "epub" -> "application/epub+zip"
        else -> "application/zip"
    }

    private fun textMime(ext: String): String = when (ext) {
        "json" -> "application/json"
        "csv" -> "text/csv"
        "md", "markdown" -> "text/markdown"
        "yaml", "yml" -> "application/yaml"
        else -> "text/plain"
    }

    /**
     * 内容嗅探：没有 NUL 字节、且能按 UTF-8 解码。
     * 只看开头一段即可 —— 攻击者可以构造"开头像文本、后面是二进制"的文件，
     * 所以这类文件最终也只会被判为 TEXT，而不会被当成图片放行。
     */
    private fun looksLikeText(head: ByteArray): Boolean {
        if (head.any { it == 0.toByte() }) return false
        val decoder = Charsets.UTF_8.newDecoder()
        return runCatching {
            decoder.decode(java.nio.ByteBuffer.wrap(head))
            true
        }.getOrDefault(false)
    }
}
