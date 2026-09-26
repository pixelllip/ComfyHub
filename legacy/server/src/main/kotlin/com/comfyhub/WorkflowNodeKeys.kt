package com.comfyhub

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * 界面格式工作流的几个**小工具**（节点 id / 类名 / 引用写法）。
 *
 * 单独一个文件是因为它们现在有两个使用者：
 *  - `WorkflowConvert`（界面格式 → API 格式）；
 *  - `FrontendNodes`（## 模拟用户行为 ##：把纯前端节点等价改写掉）。
 *
 * 这两个键名的区别是**踩过的坑**，别混：
 *  - 界面格式里类名在 **`type`**（[uiClass]）；
 *  - API 格式里类名在 **`class_type`**（[JsonObject.cls]）。
 * 第一版转换器在这里用了 `cls()`，于是每个节点都被当成"没有类名"跳过，
 * 结果是**一张空图** —— 而空图提交上去只会得到 ComfyUI 一句 "Prompt has no outputs"，
 * 完全看不出真正原因（用例因此专门盯着 `nodeCount`）。
 */

/** 节点 id 统一当**文本**用（界面格式里是数字，API 图里是对象键）。 */
internal fun JsonObject.idOf(): String? = this["id"].asText()?.takeIf { it.isNotBlank() }

/** 界面格式里节点的类名（不是 API 格式那个 `class_type`）。 */
internal fun JsonObject.uiClass(): String = this["type"].asText().orEmpty()

/**
 * 节点引用写成**数字**（节点 id 都是数字，与 ComfyUI 自己导出的一致）。
 * 万一以后出现非数字 id（理论上可能），退回文本 —— 两边的查表用的都是对象键，都认。
 */
internal fun refOf(id: String): JsonPrimitive =
    id.toLongOrNull()?.let { JsonPrimitive(it) } ?: JsonPrimitive(id)
