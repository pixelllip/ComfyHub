package com.comfyhub

import kotlinx.serialization.json.Json

/**
 * 全局 JSON 配置。宽松解析（ignoreUnknownKeys）方便前后端各自演进。
 */
val AppJson: Json = Json {
    ignoreUnknownKeys = true
    isLenient = true
    encodeDefaults = true
    explicitNulls = false
    prettyPrint = false
}

val AppJsonPretty: Json = Json {
    ignoreUnknownKeys = true
    isLenient = true
    encodeDefaults = true
    explicitNulls = false
    prettyPrint = true
}
