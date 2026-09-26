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
 * 角色卡接口（O8，2026-09-25）。
 *
 * ```
 *  GET    /api/ai/character-cards           列表（?q= 关键字 / ?limit=）
 *  POST   /api/ai/character-cards           新建
 *  GET    /api/ai/character-cards/{id}      详情
 *  PUT    /api/ai/character-cards/{id}      编辑（全量替换，界面弹窗用）
 *  DELETE /api/ai/character-cards/{id}      删除
 * ```
 *
 * 路径挂在 `/api/ai/` 下是因为**读写它的两方都是 AI 域的东西**：
 * 界面（`AiApiClient`，错误体 `{error, detail}`）与 AI 工具（`save_character_card`）。
 * 库那侧（`ApiClient`）只认 `error` 是人话 —— 混用会让界面只显示 `CONFIG_ERROR`
 * 这种机器码，而且不报错（见 `test/api_error_envelope_test.dart`）。
 *
 * **没有 `/save` 那条路**：AI 的"按名字合并保存"直接走 [CharacterCardRepo.save]
 * （不绕 HTTP），所以这里只提供界面的全量增删改查。
 *
 * ⚠ 404 的 body 会被 `Application.kt` 的 StatusPages 统一改写成
 * `{"error":"接口不存在: <路径>"}`（**这是 Kotlin 侧今天就有的全局行为**，tags / media
 * 都一样；Rust 侧刻意只对齐 body 之外的语义）。所以别照着实测的 404 文案判断
 * "路由里的 respondNotFound 没生效" —— 它生效过，是被后写的响应盖掉了。
 */
fun Route.characterCardRoutes() {
    route("/ai/character-cards") {

        get {
            val p = call.request.queryParameters
            call.respond(
                CharacterCardRepo.list(
                    q = p.str("q"),
                    limit = p.int("limit", CharacterCardRepo.MAX_SEARCH),
                )
            )
        }

        post {
            val input = call.receive<CharacterCardInput>()
            val id = Db.tx { conn -> CharacterCardRepo.create(conn, input) }
            call.respond(
                HttpStatusCode.Created,
                Db.withConnection { CharacterCardRepo.get(it, id) } ?: ApiError("创建失败"),
            )
        }

        route("/{id}") {
            get {
                val id = call.requireId() ?: return@get call.respondBadRequest("非法的 id")
                call.respond(
                    Db.withConnection { CharacterCardRepo.get(it, id) }
                        ?: ApiError("角色卡 $id 不存在"),
                )
            }

            put {
                val id = call.requireId() ?: return@put call.respondBadRequest("非法的 id")
                val input = call.receive<CharacterCardInput>()
                val ok = Db.tx { conn -> CharacterCardRepo.update(conn, id, input) }
                if (!ok) return@put call.respondNotFound("角色卡 $id 不存在")
                call.respond(Db.withConnection { CharacterCardRepo.get(it, id) } ?: ApiError("读取失败"))
            }

            delete {
                val id = call.requireId() ?: return@delete call.respondBadRequest("非法的 id")
                val ok = Db.tx { conn -> CharacterCardRepo.delete(conn, id) }
                if (!ok) return@delete call.respondNotFound("角色卡 $id 不存在")
                call.respond(mapOf("deleted" to id))
            }
        }
    }
}
