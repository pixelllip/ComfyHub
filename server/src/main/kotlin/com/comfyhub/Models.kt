package com.comfyhub

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject

// ===========================================================================
//  标签
// ===========================================================================

@Serializable
data class TagDto(
    val id: Long,
    val name: String,
    val category: String? = null,
    val color: String? = null,
    val description: String? = null,
    val useCount: Int = 0,
    val createdAt: String? = null,
)

@Serializable
data class TagInput(
    val name: String,
    val category: String? = null,
    val color: String? = null,
    val description: String? = null,
)

// ===========================================================================
//  提示词
// ===========================================================================

@Serializable
data class LoraRef(
    val name: String,
    val weight: Double = 1.0,
)

@Serializable
data class PromptDto(
    val id: Long,
    val title: String,
    val kind: String,
    val positivePrompt: String,
    val negativePrompt: String? = null,
    val checkpoint: String? = null,
    val loras: List<LoraRef> = emptyList(),
    val sampler: String? = null,
    val scheduler: String? = null,
    val steps: Int? = null,
    val cfgScale: Double? = null,
    val seed: Long? = null,
    val width: Int? = null,
    val height: Int? = null,
    val batchSize: Int? = null,
    val extraParams: Map<String, String> = emptyMap(),
    val notes: String? = null,
    val favorite: Boolean = false,
    /** 来源: Manual / ComfyUI / ComfyUI-Import */
    val source: String? = null,
    /** 来源侧唯一标识（如 ComfyUI prompt_id） */
    val sourceRef: String? = null,
    /** 是否存了完整工作流（决定前端要不要显示「查看工作流」） */
    val hasWorkflow: Boolean = false,
    val createdAt: String? = null,
    val updatedAt: String? = null,
    val tags: List<TagDto> = emptyList(),
    val mediaCount: Int = 0,
)

@Serializable
data class PromptInput(
    val title: String = "",
    val kind: String = "IMAGE",
    val positivePrompt: String = "",
    val negativePrompt: String? = null,
    val checkpoint: String? = null,
    val loras: List<LoraRef> = emptyList(),
    val sampler: String? = null,
    val scheduler: String? = null,
    val steps: Int? = null,
    val cfgScale: Double? = null,
    val seed: Long? = null,
    val width: Int? = null,
    val height: Int? = null,
    val batchSize: Int? = null,
    val extraParams: Map<String, String> = emptyMap(),
    val notes: String? = null,
    val favorite: Boolean = false,
    /** 标签名列表；后端会自动创建不存在的标签 */
    val tags: List<String> = emptyList(),
)

// ===========================================================================
//  生成产物（图片 / 视频 / 音频）
// ===========================================================================

@Serializable
data class MediaDto(
    val id: Long,
    val promptId: Long? = null,
    val kind: String,
    val title: String = "",
    val originalName: String,
    val storedName: String,
    val mimeType: String? = null,
    val sizeBytes: Long = 0,
    val width: Int? = null,
    val height: Int? = null,
    val durationMs: Long? = null,
    val sha256: String? = null,
    val source: String? = null,
    val favorite: Boolean = false,
    val notes: String? = null,
    val createdAt: String? = null,
    val updatedAt: String? = null,
    /** 这条产物是否存了工作流原文（前端据此决定要不要显示「查看工作流」） */
    val hasWorkflow: Boolean = false,

    // --- 关联提示词摘要（视图直出，前端列表无需二次请求） ---
    val promptTitle: String? = null,
    val promptPositive: String? = null,
    val promptNegative: String? = null,
    val checkpoint: String? = null,
    val seed: Long? = null,
    val promptTags: List<String> = emptyList(),

    // --- 前端直接用这两个相对路径拼 baseUrl ---
    val fileUrl: String,
    val thumbUrl: String? = null,
)

@Serializable
data class MediaUpdate(
    val promptId: Long? = null,
    /** 明确把 promptId 置空（解除关联）时为 true */
    val clearPrompt: Boolean = false,
    val title: String? = null,
    val notes: String? = null,
    val favorite: Boolean? = null,
    val source: String? = null,
)

// ===========================================================================
//  通用
// ===========================================================================

@Serializable
data class PageDto<T>(
    val items: List<T>,
    val total: Long,
    val page: Int,
    val size: Int,
    val pages: Int,
)

@Serializable
data class ApiError(
    val error: String,
    val detail: String? = null,
)

@Serializable
data class StatsDto(
    val prompts: Long,
    val media: Long,
    val tags: Long,
    val favoritePrompts: Long,
    val favoriteMedia: Long,
    val byKind: Map<String, Long>,
    val byMediaKind: Map<String, Long>,
)

@Serializable
data class TagNamesInput(
    val tags: List<String> = emptyList(),
)

@Serializable
data class FavoriteInput(
    val favorite: Boolean,
)

@Serializable
data class UploadResultDto(
    /** 本次新入库的产物 */
    val items: List<MediaDto> = emptyList(),
    /** 因为内容重复（sha256 相同）而被跳过的文件，返回已存在的产物 id */
    val duplicates: List<DuplicateInfo> = emptyList(),
    /** 失败的文件与原因 */
    val failed: List<FailedInfo> = emptyList(),
    /** 挂载到的提示词 id（若请求里带了） */
    val promptId: Long? = null,
)

@Serializable
data class DuplicateInfo(
    val fileName: String,
    val existingMediaId: Long,
)

@Serializable
data class FailedInfo(
    val fileName: String,
    val reason: String,
)

@Serializable
data class HealthDto(
    val status: String,
    val version: String,
    val database: String,
    val storageDir: String,
    val serverTime: String,
)

// ===========================================================================
//  ComfyUI 自动捕获
// ===========================================================================

/** 一个产出文件（来自 ComfyUI 的 /history outputs，或推送方的本地路径） */
@Serializable
data class IngestOutput(
    val filename: String,
    val subfolder: String? = null,
    /** output / temp；temp 默认不导入 */
    val type: String? = null,
    /** 可选：由推送方直接指定类型（IMAGE / VIDEO / AUDIO） */
    val kind: String? = null,
    /** 产生它的节点（用于判断视频 / 音频） */
    val nodeId: String? = null,
    val nodeType: String? = null,
)

@Serializable
data class IngestExtra(
    val clientId: String? = null,
    val tags: List<String> = emptyList(),
    val title: String? = null,
    val notes: String? = null,
)

/**
 * 一次生成的捕获请求。
 *
 * 三种来源都发这个格式：后端轮询 /history、ComfyUI 自定义节点推送、外部脚本。
 */
@Serializable
data class IngestRequest(
    /** 幂等键：ComfyUI 的 prompt_id */
    val runKey: String,
    /** 来源标记，默认 ComfyUI */
    val source: String = "ComfyUI",
    val status: String = "success",
    val error: String? = null,
    val comfyUrl: String? = null,
    /** ComfyUI 的输出根目录；给了就直接读本地文件，省一次 HTTP 下载 */
    val outputDir: String? = null,
    /** API 格式节点图（参数全在这里） */
    val prompt: JsonObject? = null,
    /** 界面格式工作流（可拖回 ComfyUI 复现） */
    val workflow: JsonObject? = null,
    val outputs: List<IngestOutput> = emptyList(),
    val extra: IngestExtra? = null,
    /** 原始 history 片段，仅用于排查 */
    val raw: JsonObject? = null,
)

@Serializable
data class IngestResult(
    val runKey: String,
    val promptId: Long? = null,
    val created: Boolean = false,
    val mediaIds: List<Long> = emptyList(),
    val imported: Int = 0,
    val duplicates: Int = 0,
    val failed: Int = 0,
    val alreadyCaptured: Boolean = false,
    val message: String? = null,
)

@Serializable
data class CaptureConfig(
    val enabled: Boolean = true,
    val comfyUrl: String = "http://127.0.0.1:8188",
    /** ComfyUI 输出目录，留空则全部通过 HTTP 下载 */
    val outputDir: String? = null,
    val pollSeconds: Int = 4,
    /** 自动给捕获到的提示词打上的标签 */
    val autoTag: String = "ComfyUI",
    /** 一次轮询最多处理几条未捕获的运行 */
    val maxPerPoll: Int = 20,
    /** 本地文件缺失时是否回退到 HTTP 下载 */
    val downloadFallback: Boolean = true,
)

@Serializable
data class CaptureRunInfo(
    val runKey: String,
    val promptId: Long? = null,
    val status: String = "success",
    val mediaCount: Int = 0,
    val title: String? = null,
    val error: String? = null,
    val capturedAt: String? = null,
)

@Serializable
data class CaptureStatus(
    val enabled: Boolean,
    val comfyUrl: String,
    val outputDir: String? = null,
    val comfyReachable: Boolean = false,
    val queueRunning: Int = 0,
    val queuePending: Int = 0,
    val lastPollAt: String? = null,
    val lastError: String? = null,
    val capturedRuns: Long = 0,
    val capturedMedia: Long = 0,
    val recent: List<CaptureRunInfo> = emptyList(),
)

@Serializable
data class CapturePollResult(
    val ok: Boolean,
    val checked: Int = 0,
    val newRuns: Int = 0,
    val newMedia: Int = 0,
    val message: String? = null,
)

@Serializable
data class ImportFolderRequest(
    val dir: String,
    val recursive: Boolean = true,
    val limit: Int = 200,
    /** 读取 PNG 内嵌的 prompt / workflow，自动建提示词并关联 */
    val linkWorkflow: Boolean = true,
    val tags: List<String> = emptyList(),
)

@Serializable
data class ImportFolderResult(
    val dir: String,
    val scanned: Int = 0,
    val imported: Int = 0,
    val duplicates: Int = 0,
    val failed: Int = 0,
    val promptsCreated: Int = 0,
    val promptIds: List<Long> = emptyList(),
    val errors: List<String> = emptyList(),
)

