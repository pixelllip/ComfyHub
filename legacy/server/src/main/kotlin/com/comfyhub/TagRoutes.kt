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
 * 标签接口
 *
 *  GET    /api/tags              标签列表（支持 q / category / sort）
 *  GET    /api/tags/categories   所有分类
 *  POST   /api/tags              新建
 *  PUT    /api/tags/{id}         重命名 / 改色 / 改分类
 *  DELETE /api/tags/{id}         删除（同时解除所有关联）
 */
fun Route.tagRoutes() {
    route("/tags") {

        get {
            val p = call.request.queryParameters
            call.respond(
                TagRepo.list(
                    q = p.str("q"),
                    category = p.str("category"),
                    sort = p.str("sort") ?: "popular",
                    limit = p.int("limit", 500),
                )
            )
        }

        get("/categories") {
            call.respond(TagRepo.categories())
        }

        post {
            val input = call.receive<TagInput>()
            val id = Db.tx { conn -> TagRepo.ensure(conn, input.name, input.category, input.color) }
                ?: return@post call.respondBadRequest("标签名不能为空")
            Db.tx { conn -> TagRepo.update(conn, id, input) }
            call.respond(HttpStatusCode.Created, Db.withConnection { TagRepo.getById(it, id) } ?: ApiError("创建失败"))
        }

        route("/{id}") {
            put {
                val id = call.requireId() ?: return@put call.respondBadRequest("非法的 id")
                val input = call.receive<TagInput>()
                val ok = Db.tx { conn -> TagRepo.update(conn, id, input) }
                if (!ok) return@put call.respondNotFound("标签 $id 不存在")
                call.respond(Db.withConnection { TagRepo.getById(it, id) } ?: ApiError("读取失败"))
            }

            delete {
                val id = call.requireId() ?: return@delete call.respondBadRequest("非法的 id")
                val ok = Db.tx { conn -> TagRepo.delete(conn, id) }
                if (!ok) return@delete call.respondNotFound("标签 $id 不存在")
                call.respond(mapOf("deleted" to id))
            }
        }
    }
}
