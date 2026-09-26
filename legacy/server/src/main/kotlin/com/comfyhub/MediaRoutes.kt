package com.comfyhub

import io.ktor.http.HttpStatusCode
import io.ktor.http.content.PartData
import io.ktor.http.content.forEachPart
import io.ktor.server.request.receive
import io.ktor.server.request.receiveMultipart
import io.ktor.server.response.respond
import io.ktor.server.response.respondFile
import io.ktor.server.routing.Route
import io.ktor.server.routing.delete
import io.ktor.server.routing.get
import io.ktor.server.routing.patch
import io.ktor.server.routing.post
import io.ktor.server.routing.route
import io.ktor.utils.io.jvm.javaio.toInputStream
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.slf4j.LoggerFactory
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption

private val log = LoggerFactory.getLogger("com.comfyhub.MediaRoutes")

/**
 * 生成产物接口
 *
 *  GET    /api/media                  列表 + 搜索（关键词 / 标签 / 类型 / 归属提示词）
 *  POST   /api/media/upload           上传（multipart，支持多文件）
 *  GET    /api/media/{id}             详情（含关联提示词摘要）
 *  PATCH  /api/media/{id}             改标题 / 关联提示词 / 收藏 / 备注
 *  DELETE /api/media/{id}             删除（同时删除磁盘文件）
 *  GET    /api/media/{id}/file        原始文件（支持 Range，用于视频拖动）
 *  GET    /api/media/{id}/thumb       缩略图（仅图片，ImageIO 生成）
 *  GET    /api/media/{id}/prompt      关联的完整提示词
 */
fun Route.mediaRoutes(ctx: AppContext) {
    val storage = ctx.storage

    route("/media") {

        get {
            val p = call.request.queryParameters
            call.respond(
                MediaRepo.search(
                    MediaRepo.SearchArgs(
                        q = p.str("q"),
                        tags = TagRepo.parseTagParam(p.str("tags")),
                        tagMode = p.str("tagMode") ?: "any",
                        kind = p.str("kind"),
                        promptId = p.long("promptId"),
                        favorite = p.bool("favorite"),
                        untagged = p.bool("untagged") ?: false,
                        sort = p.str("sort") ?: "newest",
                        page = p.int("page", 1),
                        size = p.int("size", 24),
                    )
                )
            )
        }

        // -------------------------------------------------------------------
        //  上传
        // -------------------------------------------------------------------
        post("/upload") {
            var promptId: Long? = null
            var title: String? = null
            var kindOverride: String? = null
            var source: String? = null
            var notes: String? = null
            var tags: List<String> = emptyList()

            data class Incoming(val originalName: String, val tmp: Path, val mime: String?, val size: Long)

            val incoming = mutableListOf<Incoming>()
            val formErrors = mutableListOf<FailedInfo>()

            try {
                call.receiveMultipart(formFieldLimit = ctx.cfg.maxUploadBytes).forEachPart { part ->
                    try {
                        when (part) {
                            is PartData.FormItem -> when (part.name) {
                                "promptId" -> promptId = part.value.trim().toLongOrNull()
                                "title" -> title = part.value.trim().takeIf { it.isNotEmpty() }
                                "kind" -> kindOverride = part.value.trim().uppercase().takeIf { it in MediaRepo.KINDS }
                                "source" -> source = part.value.trim().takeIf { it.isNotEmpty() }
                                "notes" -> notes = part.value.trim().takeIf { it.isNotEmpty() }
                                "tags" -> tags = part.value.split(',').map { it.trim() }.filter { it.isNotEmpty() }
                                else -> Unit
                            }

                            is PartData.FileItem -> {
                                val name = part.originalFileName?.takeIf { it.isNotBlank() } ?: "upload.bin"
                                val tmp = storage.tempFile()
                                val size = try {
                                    part.provider().toInputStream().use { input ->
                                        Files.newOutputStream(tmp).use { out -> input.copyTo(out) }
                                    }
                                } catch (e: Exception) {
                                    Files.deleteIfExists(tmp)
                                    throw e
                                }
                                incoming += Incoming(
                                    originalName = name,
                                    tmp = tmp,
                                    mime = part.contentType?.toString(),
                                    size = size,
                                )
                            }

                            else -> Unit
                        }
                    } catch (e: Exception) {
                        log.warn("multipart 分段处理失败: {}", e.message)
                        formErrors += FailedInfo("(part)", e.message ?: "解析失败")
                    } finally {
                        part.dispose()
                    }
                }
            } catch (e: Exception) {
                incoming.forEach { runCatching { Files.deleteIfExists(it.tmp) } }
                return@post call.respondBadRequest("上传解析失败", e.message)
            }

            if (incoming.isEmpty()) {
                return@post call.respondBadRequest("没有收到文件（字段名请用 files）")
            }

            if (promptId != null && Db.withConnection { PromptRepo.get(it, promptId!!) } == null) {
                incoming.forEach { runCatching { Files.deleteIfExists(it.tmp) } }
                return@post call.respondBadRequest("promptId=$promptId 对应的提示词不存在")
            }

            val created = mutableListOf<MediaDto>()
            val duplicates = mutableListOf<DuplicateInfo>()
            val failed = formErrors.toMutableList()

            for (item in incoming) {
                var moved: Path? = null
                val storedName = storage.newStoredName(item.originalName)
                try {
                    moved = storage.mediaDir.resolve(storedName)
                    Files.move(item.tmp, moved, StandardCopyOption.REPLACE_EXISTING)

                    val kind = kindOverride ?: MediaFiles.kindOf(item.originalName, item.mime)
                    val mime = MediaFiles.detectMime(item.originalName, item.mime)
                    val sha = MediaFiles.sha256(moved)

                    val existing = Db.withConnection { conn -> MediaRepo.findBySha(conn, sha) }
                    if (existing != null) {
                        Files.deleteIfExists(moved)
                        duplicates += DuplicateInfo(item.originalName, existing.id)
                        continue
                    }

                    val dims = if (kind == "IMAGE") MediaFiles.probeImageSize(moved) else null

                    val newId = Db.tx { conn ->
                        val id = MediaRepo.insert(
                            conn = conn,
                            kind = kind,
                            title = title ?: item.originalName,
                            originalName = item.originalName,
                            storedName = storedName,
                            mimeType = mime,
                            sizeBytes = item.size,
                            width = dims?.first,
                            height = dims?.second,
                            durationMs = null,
                            sha256 = sha,
                            source = source ?: "ComfyUI",
                            promptId = promptId,
                            notes = notes,
                        )
                        if (tags.isNotEmpty()) TagRepo.setMediaTags(conn, id, tags)
                        TagRepo.refreshUseCounts(conn)
                        id
                    }

                    if (kind == "IMAGE") {
                        MediaFiles.writeThumbnail(moved, storage.thumbPath(newId))
                    }

                    Db.withConnection { conn -> MediaRepo.get(conn, newId) }?.let { created += it }
                } catch (e: Exception) {
                    log.error("文件入库失败 {}", item.originalName, e)
                    moved?.let { runCatching { Files.deleteIfExists(it) } }
                    runCatching { Files.deleteIfExists(item.tmp) }
                    failed += FailedInfo(item.originalName, e.message ?: "入库失败")
                }
            }

            call.respond(
                HttpStatusCode.Created,
                UploadResultDto(items = created, duplicates = duplicates, failed = failed, promptId = promptId)
            )
        }

        // -------------------------------------------------------------------
        //  单条
        // -------------------------------------------------------------------
        route("/{id}") {
            get {
                val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
                val media = MediaRepo.get(id) ?: return@get call.respondNotFound("产物 $id 不存在")
                call.respond(media)
            }

            patch {
                val id = call.requireId() ?: return@patch call.respondBadRequest("非法的 id")
                val body = call.receive<MediaUpdate>()
                if (body.promptId != null && Db.withConnection { PromptRepo.get(it, body.promptId!!) } == null) {
                    return@patch call.respondBadRequest("promptId=${body.promptId} 对应的提示词不存在")
                }
                val ok = Db.tx { conn -> MediaRepo.update(conn, id, body) }
                if (!ok) return@patch call.respondNotFound("产物 $id 不存在或无字段需要更新")
                call.respond(MediaRepo.get(id) ?: ApiError("读取失败"))
            }

            delete {
                val id = call.requireId() ?: return@delete call.respondBadRequest("非法的 id")
                val media = MediaRepo.get(id) ?: return@delete call.respondNotFound("产物 $id 不存在")
                Db.tx { conn -> MediaRepo.delete(conn, id) }
                storage.deleteMediaFile(media.storedName)
                storage.deleteThumb(id)
                call.respond(mapOf("deleted" to id))
            }

            get("/file") {
                val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
                val media = MediaRepo.get(id) ?: return@get call.respondNotFound("产物 $id 不存在")
                val path = storage.resolveMedia(media.storedName)
                    ?: return@get call.respondNotFound("文件已丢失: ${media.storedName}")
                call.setInlineFileHeaders(media.originalName, contentTypeFor(media.mimeType))
                call.respondFile(path.toFile())
            }

            get("/thumb") {
                val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
                val media = MediaRepo.get(id) ?: return@get call.respondNotFound("产物 $id 不存在")
                if (media.kind != "IMAGE") return@get call.respond(HttpStatusCode.NoContent)

                val thumb = storage.thumbPath(id)
                if (!Files.isRegularFile(thumb)) {
                    val src = storage.resolveMedia(media.storedName)
                        ?: return@get call.respondNotFound("文件已丢失")
                    if (!MediaFiles.writeThumbnail(src, thumb)) {
                        // 生成失败（例如 webp/avif 无解码器）时回退到原图
                        call.setInlineFileHeaders(media.originalName, contentTypeFor(media.mimeType))
                        return@get call.respondFile(src.toFile())
                    }
                }
                call.response.headers.append("X-Thumbnail", "1")
                call.setInlineFileHeaders("thumb-$id.jpg", contentTypeFor("image/jpeg"))
                call.respondFile(thumb.toFile())
            }

            get("/poster") {
                val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
                val media = MediaRepo.get(id) ?: return@get call.respondNotFound("产物 $id 不存在")
                // 只有视频需要"第一帧封面"；图片本来就有缩略图，音频没有画面
                if (media.kind != "VIDEO") return@get call.respond(HttpStatusCode.NoContent)

                val poster = storage.posterPath(id)
                if (!Files.isRegularFile(poster)) {
                    val src = storage.resolveMedia(media.storedName)
                        ?: return@get call.respondNotFound("文件已丢失")
                    // 抽帧要起一个子进程，放到 IO 线程上，别占着事件循环
                    val ok = withContext(Dispatchers.IO) {
                        MediaFiles.writeVideoPoster(src, poster, roots = listOf(ctx.storage.mediaDir))
                    }
                    // 抽不出来就明确告诉前端"没有封面"（204），播放器会转圈而不是显示破图
                    if (!ok) return@get call.respond(HttpStatusCode.NoContent)
                }
                call.response.headers.append("X-Poster", "1")
                call.setInlineFileHeaders("poster-$id.png", contentTypeFor("image/png"))
                call.respondFile(poster.toFile())
            }

            get("/prompt") {
                val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
                val media = MediaRepo.get(id) ?: return@get call.respondNotFound("产物 $id 不存在")
                val pid = media.promptId ?: return@get call.respond(HttpStatusCode.NoContent)
                call.respond(PromptRepo.get(pid) ?: ApiError("关联的提示词已被删除"))
            }
        }
    }
}
