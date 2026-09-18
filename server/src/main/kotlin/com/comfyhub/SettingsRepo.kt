package com.comfyhub

import org.slf4j.LoggerFactory
import java.sql.Connection

/**
 * 运行期设置（`app_settings` 表，k/v）。
 *
 * 自动捕获的开关、ComfyUI 地址、输出目录这些，既要在 App 的设置页里改，
 * 也要被后端的轮询线程读到 —— 所以放进数据库，两边共用同一份，避免两边配置打架。
 */
object SettingsRepo {
    private val log = LoggerFactory.getLogger(SettingsRepo::class.java)

    private const val KEY_CAPTURE = "capture.config"

    // -----------------------------------------------------------------------
    //  通用 k/v
    // -----------------------------------------------------------------------

    fun get(key: String): String? =
        Db.withConnection { conn -> conn.queryOne("SELECT v FROM app_settings WHERE k = ?", key) { it.getString(1) } }

    fun put(key: String, value: String?) = Db.withConnection { conn ->
        conn.execute(
            """
            INSERT INTO app_settings (k, v) VALUES (?, ?)
            ON DUPLICATE KEY UPDATE v = VALUES(v)
            """.trimIndent(), key, value
        )
        Unit
    }

    // -----------------------------------------------------------------------
    //  自动捕获配置
    // -----------------------------------------------------------------------

    fun captureConfig(cfg: AppConfig): CaptureConfig {
        val raw = get(KEY_CAPTURE) ?: return CaptureConfig(
            comfyUrl = cfg.comfyUrl,
            outputDir = cfg.comfyOutputDir,
        )
        return runCatching { AppJson.decodeFromString(CaptureConfig.serializer(), raw) }
            .getOrElse {
                log.warn("解析 capture.config 失败，回退默认值: {}", it.message)
                CaptureConfig(comfyUrl = cfg.comfyUrl, outputDir = cfg.comfyOutputDir)
            }
    }

    /**
     * 只回答"用户配过产物目录没有"，**不做默认值回填**。
     *
     * 给 [ComfyRoots] 的目录探测用：那里需要的是"用户实际在用的那个 ComfyUI 在哪"，
     * 而不是 `AppConfig` 里的出厂默认值（发布包的默认值跟用户机器上的安装位置毫无关系）。
     */
    fun captureOutputDirOrNull(): String? = get(KEY_CAPTURE)
        ?.let { raw ->
            runCatching { AppJson.decodeFromString(CaptureConfig.serializer(), raw).outputDir }.getOrNull()
        }
        ?.takeIf { it.isNotBlank() }

    fun saveCaptureConfig(config: CaptureConfig): CaptureConfig {
        val normalized = config.copy(
            comfyUrl = config.comfyUrl.trim().trimEnd('/').ifBlank { "http://127.0.0.1:8188" },
            outputDir = config.outputDir?.trim()?.takeIf { it.isNotEmpty() },
            pollSeconds = config.pollSeconds.coerceIn(1, 600),
            maxPerPoll = config.maxPerPoll.coerceIn(1, 500),
            autoTag = config.autoTag.trim(),
        )
        put(KEY_CAPTURE, AppJson.encodeToString(CaptureConfig.serializer(), normalized))
        return normalized
    }

    /** 直接落一个 SQL 更新，供连接内使用 */
    fun <T> inTx(block: (Connection) -> T): T = Db.withConnection(block)
}
