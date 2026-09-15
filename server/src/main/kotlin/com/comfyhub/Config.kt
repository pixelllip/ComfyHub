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
    /**
     * 允许被局域网访问（AIH-016）。
     *
     * 后端持有 AI Provider 的 API Key 并会代替用户花钱；一旦监听 `0.0.0.0`，
     * 同一局域网里任何人都能用你的额度。所以默认只监听回环地址，
     * 必须显式打开 `COMFYHUB_ALLOW_REMOTE=1`（远程/移动端场景）才能对外。
     */
    val allowRemote: Boolean = false,
    /** CORS 允许的 Origin 白名单（AIH-016）；空表示只允许本机来源 */
    val corsOrigins: List<String> = emptyList(),
) {
    /** 是否只监听回环地址 */
    val loopbackOnly: Boolean get() = host == "127.0.0.1" || host == "::1" || host == "localhost"

    companion object {
        fun fromEnv(): AppConfig {
            val env = System.getenv()
            fun get(key: String, default: String): String =
                env[key]?.takeIf { it.isNotBlank() } ?: default

            val storage = Paths.get(
                get("COMFYHUB_STORAGE", defaultStorageDir())
            ).toAbsolutePath().normalize()

            // 默认只监听回环。想被局域网访问必须显式 COMFYHUB_ALLOW_REMOTE=1；
            // 也可以直接给 COMFYHUB_HOST 覆盖（此时以显式值为准）。
            val allowRemote = get("COMFYHUB_ALLOW_REMOTE", "0").let { it == "1" || it.equals("true", true) }
            val explicitHost = env["COMFYHUB_HOST"]?.takeIf { it.isNotBlank() }
            val host = explicitHost ?: if (allowRemote) "0.0.0.0" else "127.0.0.1"

            val corsOrigins = get("COMFYHUB_CORS_ORIGINS", "")
                .split(',')
                .map { it.trim().trimEnd('/') }
                .filter { it.isNotBlank() }

            return AppConfig(
                host = host,
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
                allowRemote = allowRemote,
                corsOrigins = corsOrigins,
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
        append("监听范围=${if (loopbackOnly) "仅本机回环" else "对外（局域网可达）"}\n")
        if (corsOrigins.isNotEmpty()) append("CORS 白名单=${corsOrigins.joinToString(",")}\n")
        append("jdbcUrl=$jdbcUrl\n")
        append("dbUser=$dbUser\n")
        append("storage=$storageDir\n")
        append("maxUploadMB=${maxUploadBytes / 1024 / 1024}\n")
        append("comfyUrl=$comfyUrl\n")
        append("comfyOutput=${comfyOutputDir ?: "(未设置，走 HTTP 下载)"}\n")
    }
}
