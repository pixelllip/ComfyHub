package com.example.viewer

import android.Manifest
import android.app.Activity
import android.content.ContentValues
import android.content.pm.PackageManager
import android.media.MediaScannerConnection
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.File

/**
 * 把手机缓存里的**单条产物**保存进系统相册 / 媒体库。
 *
 * 与桌面端的「导出产物」是两件事：那边写桌面磁盘、带工作流与集合；
 * 这边只把用户选中的这一条复制进 Android 的 MediaStore（相册里立刻可见）。
 *
 * Android 10+ 用 `RELATIVE_PATH` + `IS_PENDING`（分区存储的正路，不需要权限）；
 * Android 9 及以下只能写公共 Pictures/Movies/Music，需要 `WRITE_EXTERNAL_STORAGE`，
 * 没授权时如实拒绝并请求一次 —— 不静默失败。
 */
class MediaSaver(private val activity: Activity) : MethodChannel.MethodCallHandler {

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "saveToGallery" -> save(
                path = call.argument<String>("path").orEmpty(),
                mime = call.argument<String>("mime").orEmpty(),
                name = call.argument<String>("name").orEmpty(),
                result = result,
            )
            else -> result.notImplemented()
        }
    }

    private fun save(path: String, mime: String, name: String, result: MethodChannel.Result) {
        val file = File(path)
        if (path.isBlank() || !file.isFile || file.length() <= 0L) {
            result.error("FILE_MISSING", "要保存的文件不存在或为空：$path", null)
            return
        }
        val safeName = sanitizeName(name.ifBlank { file.name })
        val safeMime = mime.ifBlank { mimeFor(safeName) }
        // 大文件复制不要占主线程；Result 必须回主线程。
        Thread {
            try {
                val uri = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    saveViaMediaStore(file, safeMime, safeName)
                } else {
                    saveLegacy(file, safeMime, safeName)
                }
                activity.runOnUiThread { result.success(uri.toString()) }
            } catch (e: SecurityException) {
                activity.runOnUiThread {
                    result.error("PERMISSION", "没有写入相册的权限：${e.message}", null)
                }
            } catch (e: Throwable) {
                activity.runOnUiThread {
                    result.error("SAVE_FAILED", "${e::class.simpleName}: ${e.message}", null)
                }
            }
        }.start()
    }

    private fun saveViaMediaStore(file: File, mime: String, name: String): Uri {
        val resolver = activity.contentResolver
        val collection = when {
            mime.startsWith("video/") -> MediaStore.Video.Media.EXTERNAL_CONTENT_URI
            mime.startsWith("audio/") -> MediaStore.Audio.Media.EXTERNAL_CONTENT_URI
            else -> MediaStore.Images.Media.EXTERNAL_CONTENT_URI
        }
        val relative = when {
            mime.startsWith("video/") -> "${Environment.DIRECTORY_MOVIES}/ComfyHub"
            mime.startsWith("audio/") -> "${Environment.DIRECTORY_MUSIC}/ComfyHub"
            else -> "${Environment.DIRECTORY_PICTURES}/ComfyHub"
        }
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(MediaStore.MediaColumns.MIME_TYPE, mime)
            put(MediaStore.MediaColumns.RELATIVE_PATH, relative)
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = resolver.insert(collection, values)
            ?: throw IllegalStateException("MediaStore 没有返回可写位置")
        try {
            resolver.openOutputStream(uri).use { out ->
                requireNotNull(out) { "打不开 MediaStore 输出流" }
                file.inputStream().use { input -> input.copyTo(out) }
            }
            values.clear()
            values.put(MediaStore.MediaColumns.IS_PENDING, 0)
            resolver.update(uri, values, null, null)
            return uri
        } catch (e: Throwable) {
            // 失败就把半截媒体删掉，别在相册里留一个打不开的条目
            runCatching { resolver.delete(uri, null, null) }
            throw e
        }
    }

    /**
     * Android 9 及以下：公共目录 + MediaScanner。
     *
     * 权限在调用前检查；没有就请求一次并报错，**不假装成功**。
     */
    private fun saveLegacy(file: File, mime: String, name: String): Uri {
        if (activity.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            activity.runOnUiThread {
                activity.requestPermissions(
                    arrayOf(Manifest.permission.WRITE_EXTERNAL_STORAGE),
                    REQUEST_WRITE_STORAGE,
                )
            }
            throw SecurityException("请先允许「存储」权限，再点一次保存")
        }
        val typeDir = when {
            mime.startsWith("video/") -> Environment.DIRECTORY_MOVIES
            mime.startsWith("audio/") -> Environment.DIRECTORY_MUSIC
            else -> Environment.DIRECTORY_PICTURES
        }
        val dir = File(Environment.getExternalStoragePublicDirectory(typeDir), "ComfyHub")
        if (!dir.exists() && !dir.mkdirs()) throw IllegalStateException("建不了目录：${dir.absolutePath}")
        val target = File(dir, name)
        file.inputStream().use { input ->
            target.outputStream().use { output -> input.copyTo(output) }
        }
        MediaScannerConnection.scanFile(
            activity,
            arrayOf(target.absolutePath),
            arrayOf(mime),
            null,
        )
        return Uri.fromFile(target)
    }

    private fun sanitizeName(raw: String): String {
        val cleaned = raw.replace(Regex("[\\\\/:*?\"<>|]"), "_").trim()
        return cleaned.ifBlank { "comfyhub-${System.currentTimeMillis()}" }
    }

    private fun mimeFor(name: String): String = when (name.substringAfterLast('.', "").lowercase()) {
        "png" -> "image/png"
        "webp" -> "image/webp"
        "gif" -> "image/gif"
        "mp4" -> "video/mp4"
        "webm" -> "video/webm"
        "mov" -> "video/quicktime"
        "mp3" -> "audio/mpeg"
        "wav" -> "audio/wav"
        "ogg" -> "audio/ogg"
        else -> "image/jpeg"
    }

    companion object {
        const val CHANNEL = "comfyhub/media"
        private const val REQUEST_WRITE_STORAGE = 4101

        fun attach(messenger: BinaryMessenger, activity: Activity) {
            MethodChannel(messenger, CHANNEL).setMethodCallHandler(MediaSaver(activity))
        }
    }
}
