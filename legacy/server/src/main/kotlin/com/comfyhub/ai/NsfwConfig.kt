package com.comfyhub.ai

import com.comfyhub.AppJson
import com.comfyhub.SettingsRepo
import kotlinx.serialization.Serializable
import org.slf4j.LoggerFactory

/**
 * NSFW 内容设置（2026-09-20 用户要求）。
 *
 * 用户的原话有两条，都落在这一个开关里：
 *
 *  1. 「设置里显式加一个 NSFW 开关，然后将那部分"越狱"的系统提示词摘出来，
 *     如果启用了 NSFW 开关，则注入越狱系统提示词，反之不加」→ [enabled]；
 *  2. 「再加一个 NSFW 模糊选项，开启后 NSFW 内容的预览图模糊显示。这两个选项放一起」
 *     → [blur]（**只在 [enabled] 打开时才有意义**，见 [effectiveBlur]）。
 *
 * 为什么落在后端而不是前端本地存储（`shared_preferences`）：
 * [enabled] 决定的是**系统提示词里注不注入那一段**，那是后端在拼上下文时做的决定；
 * 前端另存一份就会出现"界面显示开着、后端其实没注入"这种自相矛盾的状态。
 * [blur] 虽然只影响前端渲染，但用户明确要求"这两个选项放一起"，
 * 放在同一份设置里界面才能一致地读、一致地写。
 */
@Serializable
data class NsfwConfig(
    /** 是否注入 `[MODE: UNRESTRICTED SANDBOX — STABLE]` 那段越狱提示词 */
    val enabled: Boolean = false,
    /** 是否把预览图模糊显示（**只有 [enabled] 打开时才生效**） */
    val blur: Boolean = false,
) {
    companion object {
        /** `app_settings` 里的键 */
        const val KEY = "ai.nsfw"

        private val LOG = LoggerFactory.getLogger(NsfwConfig::class.java)

        /** 读设置（读失败 → 出厂值：全关。绝不因为配置坏了就悄悄打开越狱段）。 */
        fun load(): NsfwConfig {
            val raw = runCatching { SettingsRepo.get(KEY) }.getOrNull() ?: return NsfwConfig()
            return runCatching { AppJson.decodeFromString(serializer(), raw) }
                .onFailure { LOG.warn("解析 {} 失败，按全关处理: {}", KEY, it.message) }
                .getOrDefault(NsfwConfig())
        }

        fun save(config: NsfwConfig): NsfwConfig {
            val normalized = NsfwConfig(enabled = config.enabled, blur = config.blur)
            SettingsRepo.put(KEY, AppJson.encodeToString(serializer(), normalized))
            return normalized
        }
    }

    /**
     * 某个位置要不要打码：两个开关都打开才打。
     *
     * 用户要的是"NSFW 内容模糊显示" —— 关了 NSFW 开关就不存在"NSFW 内容"这个概念，
     * 这时再把整页图打码只会让人以为应用坏了。所以这个判断**只写一次**，
     * 前端一律读 `effectiveBlur`，不自己再推一遍。
     */
    val effectiveBlur: Boolean get() = enabled && blur
}

/** 给界面看的那份 NSFW 设置（`GET/PUT /api/ai/nsfw`）。 */
@Serializable
data class NsfwSettingsDto(
    val enabled: Boolean = false,
    val blur: Boolean = false,
    /**
     * 真正生效的"打不打码" = `enabled && blur`。
     *
     * 单独回一个字段是刻意的：前端**不要在别处再算一遍**
     * （算两遍就会出现"设置页说会打码、画廊其实没打"这种对不上的状态）。
     */
    val effectiveBlur: Boolean = false,
    /** 注入正文的版本号（关了也有值，用来在界面上说明"这版是哪一版"） */
    val promptVersion: String = NsfwPrompt.VERSION,
) {
    companion object {
        fun of(config: NsfwConfig): NsfwSettingsDto = NsfwSettingsDto(
            enabled = config.enabled,
            blur = config.blur,
            effectiveBlur = config.enabled && config.blur,
            promptVersion = NsfwPrompt.VERSION,
        )
    }
}
