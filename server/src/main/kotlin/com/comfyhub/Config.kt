package com.comfyhub

import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths

/**
 * 应用配置。全部可通过环境变量覆盖，便于本机 / 服务器部署。
 */
data class AppConfig(
    val host: String,
    val port: Int,
    val jdbcUrl: String,
    val dbUser: String,
    val dbPassword: String,
    val storageDir: Path,
    val maxUploadBytes: Long,
    /** ComfyUI 默认地址（自动捕获轮询用，之后可在 App 设置里改） */
    val comfyUrl: String = "http://127.0.0.1:8188",
    /** ComfyUI 输出目录的默认值，留空表示走 HTTP 下载 */
    val comfyOutputDir: String? = null,
) {
    companion object {
        fun fromEnv(): AppConfig {
            val env = System.getenv()
            fun get(key: String, default: String): String =
                env[key]?.takeIf { it.isNotBlank() } ?: default

            val storage = Paths.get(
                get("COMFYHUB_STORAGE", defaultStorageDir())
            ).toAbsolutePath().normalize()

            return AppConfig(
                host = get("COMFYHUB_HOST", "0.0.0.0"),
                port = get("COMFYHUB_PORT", "8080").toInt(),
                jdbcUrl = get(
                    "COMFYHUB_JDBC_URL",
                    "jdbc:mysql://127.0.0.1:3307/comfy_hub" +
                        "?useUnicode=true&characterEncoding=UTF-8" +
                        "&allowPublicKeyRetrieval=true&useSSL=false&rewriteBatchedStatements=true"
                ),
                dbUser = get("COMFYHUB_DB_USER", "comfyhub"),
                dbPassword = get("COMFYHUB_DB_PASSWORD", "comfyhub"),
                storageDir = storage,
                maxUploadBytes = get("COMFYHUB_MAX_UPLOAD_MB", "4096").toLong() * 1024 * 1024,
                comfyUrl = get("COMFYHUB_COMFY_URL", "http://127.0.0.1:8188").trimEnd('/'),
                comfyOutputDir = get("COMFYHUB_COMFY_OUTPUT", "").takeIf { it.isNotBlank() },
            )
        }

        private fun defaultStorageDir(): String {
            // 优先：运行目录（server/）下的 storage/
            val cwd = Paths.get("").toAbsolutePath().normalize()
            val candidate = cwd.resolve("storage").normalize()

            // 兜底：如果运行目录明显不对（比如被当成 Windows 系统目录），
            // 就退回用户目录，避免把生成产物写进 System32。
            val asString = candidate.toString().lowercase()
            val looksWrong = asString.contains("\\windows\\") ||
                asString.contains("\\system32") ||
                asString.contains("\\driverstore\\")
            if (looksWrong) {
                val home = System.getProperty("user.home") ?: "."
                return Paths.get(home, ".comfyhub", "storage").toString()
            }
            return candidate.toString()
        }
    }

    fun ensureStorage(): Path {
        Files.createDirectories(storageDir)
        Files.createDirectories(storageDir.resolve("media"))
        Files.createDirectories(storageDir.resolve("tmp"))
        return storageDir
    }

    /** 不打印密码的展示串 */
    fun describe(): String = buildString {
        append("host=$host port=$port\n")
        append("jdbcUrl=$jdbcUrl\n")
        append("dbUser=$dbUser\n")
        append("storage=$storageDir\n")
        append("maxUploadMB=${maxUploadBytes / 1024 / 1024}\n")
        append("comfyUrl=$comfyUrl\n")
        append("comfyOutput=${comfyOutputDir ?: "(未设置，走 HTTP 下载)"}\n")
    }
}
