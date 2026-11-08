package com.comfyhub.harness.store

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * 桥把一条本地会话列给 Flutter 时用的 JSON（`listConversations` 的每一行）。
 *
 * ⚠ **`messageCount` 不是可选项**：Dart 侧 `AiConversation.messageCount` 缺省是 0，
 * 冷启动的"空壳复用 / 清理"一旦只看这个字段，就会把**有内容的会话**当成空壳删掉
 * （2026-11-08 真机：一条 12 条消息的会话被删，列表里只剩一条空的新对话）。
 * `archived` 同理：缺了会被当作未归档，已归档的会话反而冒出来。
 *
 * 单独抽出来是为了让 `:harness` 的 JUnit 能直接盯住这组字段。
 */
fun conversationJson(record: ConversationRecord, messageCount: Int): JsonObject =
    buildJsonObject {
        put("id", record.id)
        put("title", record.title)
        put("providerId", record.providerId)
        put("modelId", record.modelId)
        put("archived", record.archived)
        put("createdAt", record.createdAt)
        put("updatedAt", record.updatedAt)
        put("messageCount", messageCount)
    }
