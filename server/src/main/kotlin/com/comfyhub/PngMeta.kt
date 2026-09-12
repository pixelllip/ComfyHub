package com.comfyhub

import org.slf4j.LoggerFactory
import java.io.BufferedInputStream
import java.io.DataInputStream
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.util.zip.InflaterInputStream

/**
 * 读取 PNG 里的文本块。
 *
 * ComfyUI 保存 PNG 时会往里面塞两个 JSON：
 *   · `prompt`   —— API 格式节点图（= 提交给 /prompt 的那份参数）
 *   · `workflow` —— 界面格式工作流（可以直接拖回 ComfyUI 复现）
 *
 * 有了它，「导入历史产物」就不依赖 ComfyUI 的 /history 接口 —— 那些在装 ComfyHub
 * 之前就已经生成好的图，也能把提示词和工作流一并捞回来。
 *
 * 只支持 PNG（ComfyUI 的图片输出默认就是 PNG），其余格式返回空 Map。
 */
object PngMeta {
    private val log = LoggerFactory.getLogger(PngMeta::class.java)

    private val SIGNATURE = byteArrayOf(
        0x89.toByte(), 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A
    )

    /** 单个块的体积上限，防止畸形文件把内存吃光 */
    private const val MAX_CHUNK = 96L * 1024 * 1024

    fun readTextChunks(path: Path): Map<String, String> {
        val out = LinkedHashMap<String, String>()
        try {
            DataInputStream(BufferedInputStream(Files.newInputStream(path), 1 shl 16)).use { input ->
                val sig = ByteArray(8)
                if (input.readNBytes(sig, 0, 8) < 8 || !sig.contentEquals(SIGNATURE)) return emptyMap()

                while (true) {
                    val len = try {
                        input.readInt()
                    } catch (_: Exception) {
                        break
                    }
                    val type = ByteArray(4)
                    if (input.readNBytes(type, 0, 4) < 4) break
                    val name = String(type, StandardCharsets.US_ASCII)

                    if (len < 0 || len.toLong() > MAX_CHUNK) break
                    val data = ByteArray(len)
                    if (len > 0 && input.readNBytes(data, 0, len) < len) break
                    // CRC（4 字节）不校验，ComfyUI 写出来的都是合法的
                    try {
                        input.skipNBytes(4)
                    } catch (_: Exception) {
                        break
                    }

                    when (name) {
                        "tEXt" -> parseTEXt(data, out)
                        "zTXt" -> parseZTXt(data, out)
                        "iTXt" -> parseITXt(data, out)
                    }
                    if (name == "IEND") break
                }
            }
        } catch (e: Exception) {
            log.debug("读取 PNG 文本块失败 {}: {}", path.fileName, e.message)
        }
        return out
    }

    // -----------------------------------------------------------------------

    private fun indexOfZero(data: ByteArray, from: Int = 0): Int {
        var i = from
        while (i < data.size) {
            if (data[i] == 0.toByte()) return i
            i++
        }
        return -1
    }

    /** tEXt: keyword \0 text(Latin-1，ComfyUI 实际写 UTF-8) */
    private fun parseTEXt(data: ByteArray, out: MutableMap<String, String>) {
        val sep = indexOfZero(data)
        if (sep <= 0) return
        val key = String(data, 0, sep, StandardCharsets.ISO_8859_1)
        val value = String(data, sep + 1, data.size - sep - 1, StandardCharsets.UTF_8)
        put(out, key, value)
    }

    /** zTXt: keyword \0 method(1) zlib(text) */
    private fun parseZTXt(data: ByteArray, out: MutableMap<String, String>) {
        val sep = indexOfZero(data)
        if (sep <= 0 || sep + 2 > data.size) return
        val key = String(data, 0, sep, StandardCharsets.ISO_8859_1)
        val compressed = data.copyOfRange(sep + 2, data.size)
        inflate(compressed)?.let { put(out, key, it) }
    }

    /** iTXt: keyword \0 compFlag(1) compMethod(1) lang \0 translated \0 text */
    private fun parseITXt(data: ByteArray, out: MutableMap<String, String>) {
        val sep = indexOfZero(data)
        if (sep <= 0 || sep + 2 > data.size) return
        val key = String(data, 0, sep, StandardCharsets.ISO_8859_1)
        val compressed = data[sep + 1].toInt() == 1
        var p = sep + 3
        val langEnd = indexOfZero(data, p)
        if (langEnd < 0) return
        p = langEnd + 1
        val transEnd = indexOfZero(data, p)
        if (transEnd < 0) return
        p = transEnd + 1
        if (p > data.size) return
        val body = data.copyOfRange(p, data.size)
        val text = if (compressed) inflate(body) else String(body, StandardCharsets.UTF_8)
        text?.let { put(out, key, it) }
    }

    private fun put(out: MutableMap<String, String>, key: String, value: String) {
        // prompt / workflow 可能同时存在 tEXt 与 iTXt，保留先到的即可
        if (key.isBlank() || value.isBlank()) return
        out.putIfAbsent(key.trim(), value)
    }

    private fun inflate(data: ByteArray): String? = try {
        InflaterInputStream(data.inputStream()).use { it.readBytes().toString(StandardCharsets.UTF_8) }
    } catch (e: Exception) {
        log.debug("解压 PNG 文本块失败: {}", e.message)
        null
    }
}
