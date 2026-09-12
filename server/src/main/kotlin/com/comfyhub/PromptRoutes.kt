package com.comfyhub

import io.ktor.http.HttpStatusCode
import io.ktor.server.request.receive
import io.ktor.server.response.respond
import io.ktor.server.routing.Route
import io.ktor.server.routing.delete
import io.ktor.server.routing.get
import io.ktor.server.routing.post
import io.ktor.server.routing.put
import io.ktor.server.routing.route

/**
 * 提示词相关接口
 *
 *  GET    /api/prompts                  列表 + 搜索（关键词 / 标签 / 类型 / 收藏）
 *  POST   /api/prompts                  新建
 *  GET    /api/prompts/{id}             详情（含标签）
 *  PUT    /api/prompts/{id}             全量更新
 *  DELETE /api/prompts/{id}             删除
 *  POST   /api/prompts/{id}/duplicate   复制
 *  POST   /api/prompts/{id}/favorite    收藏 / 取消收藏
 *  POST   /api/prompts/{id}/tags        追加标签
 *  DELETE /api/prompts/{id}/tags/{tagId} 移除标签
 *  GET    /api/prompts/{id}/media       该提示词下的全部产物
 */
fun Route.promptRoutes() {
    route("/prompts") {

        get {
            val p = call.request.queryParameters
            val args = PromptRepo.SearchArgs(
                q = p.str("q"),
                tags = TagRepo.parseTagParam(p.str("tags")),
                tagMode = p.str("tagMode") ?: "any",
                kind = p.str("kind"),
                favorite = p.bool("favorite"),
                hasMedia = p.bool("hasMedia"),
                sort = p.str("sort") ?: "newest",
                page = p.int("page", 1),
                size = p.int("size", 20),
            )
            call.respond(PromptRepo.search(args))
        }

        post {
            val input = call.receive<PromptInput>()
            val id = Db.tx { conn -> PromptRepo.create(conn, input) }
            val created = PromptRepo.get(id)
            call.respond(HttpStatusCode.Created, created ?: ApiError("创建失败"))
        }

        route("/{id}") {
            get {
                val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
                val prompt = PromptRepo.get(id) ?: return@get call.respondNotFound("提示词 $id 不存在")
                call.respond(prompt)
            }

            put {
                val id = call.requireId() ?: return@put call.respondBadRequest("非法的 id")
                val input = call.receive<PromptInput>()
                val ok = Db.tx { conn -> PromptRepo.update(conn, id, input) }
                if (!ok) return@put call.respondNotFound("提示词 $id 不存在")
                call.respond(PromptRepo.get(id) ?: ApiError("更新后读取失败"))
            }

            delete {
                val id = call.requireId() ?: return@delete call.respondBadRequest("非法的 id")
                val ok = Db.tx { conn -> PromptRepo.delete(conn, id) }
                if (!ok) return@delete call.respondNotFound("提示词 $id 不存在")
                call.respond(mapOf("deleted" to id))
            }

            post("/duplicate") {
                val id = call.requireId() ?: return@post call.respondBadRequest("非法的 id")
                val newId = Db.tx { conn -> PromptRepo.duplicate(conn, id) }
                    ?: return@post call.respondNotFound("提示词 $id 不存在")
                call.respond(HttpStatusCode.Created, PromptRepo.get(newId) ?: ApiError("复制失败"))
            }

            post("/favorite") {
                val id = call.requireId() ?: return@post call.respondBadRequest("非法的 id")
                val body = call.receive<FavoriteInput>()
                val ok = Db.withConnection { conn -> PromptRepo.setFavorite(conn, id, body.favorite) }
                if (!ok) return@post call.respondNotFound("提示词 $id 不存在")
                call.respond(mapOf("id" to id, "favorite" to body.favorite))
            }

            post("/tags") {
                val id = call.requireId() ?: return@post call.respondBadRequest("非法的 id")
                val body = call.receive<TagNamesInput>()
                if (PromptRepo.get(id) == null) return@post call.respondNotFound("提示词 $id 不存在")
                Db.tx { conn ->
                    TagRepo.addPromptTags(conn, id, body.tags)
                    TagRepo.refreshUseCounts(conn)
                }
                call.respond(PromptRepo.get(id) ?: ApiError("读取失败"))
            }

            delete("/tags/{tagId}") {
                val id = call.requireId() ?: return@delete call.respondBadRequest("非法的 id")
                val tagId = call.parameters["tagId"]?.toLongOrNull()
                    ?: return@delete call.respondBadRequest("非法的 tagId")
                Db.tx { conn ->
                    TagRepo.removePromptTag(conn, id, tagId)
                    TagRepo.refreshUseCounts(conn)
                }
                call.respond(PromptRepo.get(id) ?: ApiError("读取失败"))
            }

            get("/media") {
                val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
                call.respond(MediaRepo.listByPrompt(id))
            }
        }
    }
}
