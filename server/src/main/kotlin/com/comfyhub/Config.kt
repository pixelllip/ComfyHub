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
    /**
     * 项目根目录（M4 工具权限的基准）。
     *
     * AI 的文件工具默认只能写 `<根>\comfyui`、只能读 `<根>\comfyui` 与 `<根>\storage`，
     * 所以"根"必须是**算出来的、可验证的**，不能靠当前工作目录碰运气。
     */
    val projectRoot: Path = storageDir.parent ?: storageDir,
) {
    /**
     * 底层库方言 —— **从 JDBC URL 派生**（唯一真源，别再加一个会跟它漂移的字段）。
     *
     * P2（2026-09-23）之后默认是 SQLite（`jdbc:sqlite:<根>\.db\comfyhub.db`）；
     * `jdbc:mysql://…` 只在"把老库搬进 SQLite"时作为**只读来源**出现。
     */
    val dbDialect: DbDialect
        get() = if (jdbcUrl.startsWith("jdbc:sqlite:", ignoreCase = true)) {
            DbDialect.SQLITE
        } else {
            DbDialect.MYSQL
        }

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

            // 项目根要先算出来：SQLite 库文件的默认位置就在它下面（`<根>\.db\comfyhub.db`）。
            val projectRoot = resolveProjectRoot(storage)

            return AppConfig(
                host = host,
                // 端口默认 **0 = 让操作系统挑一个空闲端口**（用户 2026-09-22）。
                //
                // 起因是本机 8080 落在 Windows 的保留端口段里（Hyper-V/WSL 预留），
                // 连 `netstat` 都看不见"被谁占了"，后端一启动就是 BindException。
                // 现在：显式值优先（`-Port` / `COMFYHUB_PORT` / 上次记录在
                // `<项目根>\.run\api.json` 里的端口由脚本传进来），
                // 没给就让 OS 挑；挑不到候选端口时 [startServer] 还会回落一次随机端口。
                // 真实端口写进 `.run\api.json`，前端 / 脚本 / ComfyUI 扩展都读那一份。
                port = parsePort(env["COMFYHUB_PORT"]),
                jdbcUrl = defaultJdbcUrl(projectRoot),
                dbUser = get("COMFYHUB_DB_USER", "comfyhub"),
                dbPassword = get("COMFYHUB_DB_PASSWORD", "comfyhub"),
                storageDir = storage,
                maxUploadBytes = get("COMFYHUB_MAX_UPLOAD_MB", "4096").toLong() * 1024 * 1024,
                comfyUrl = get("COMFYHUB_COMFY_URL", "http://127.0.0.1:8188").trimEnd('/'),
                comfyOutputDir = get("COMFYHUB_COMFY_OUTPUT", "").takeIf { it.isNotBlank() },
                allowRemote = allowRemote,
                corsOrigins = corsOrigins,
                projectRoot = projectRoot,
            )
        }

        /**
         * 解析 `COMFYHUB_PORT`。
         *
         * 四种输入都要认，而且**一个都不许让后端起不来**：
         *  · 没给 / 空 → `0`（= 让 OS 挑一个空闲端口）；
         *  · `0` → 同上（显式要求随机）；
         *  · 正整数 → 用它当候选（绑不上时由 [startServer] 回落成随机，见那里的注释）；
         *  · 乱七八糟的字符串（`"8080 "` / `"http://…"` / `"abc"`）→ 也当成 0。
         *    过去这里直接 `toInt()`，一个错字就让后端**崩在解析阶段**，
         *    而报错信息只是 `NumberFormatException` —— 用户根本看不出是自己设置写错了。
         */
        internal fun parsePort(raw: String?): Int {
            val text = raw?.trim().orEmpty()
            if (text.isEmpty()) return 0
            return text.toIntOrNull()?.takeIf { it in 0..65535 } ?: 0
        }

        /**
         * 项目根解析（`scripts\server.ps1` 会显式给 COMFYHUB_PROJECT_ROOT）。
         *
         * 顺序：环境变量 → `<storage 的父目录>`（源码树 / 发布包都是这个形状）→ 当前工作目录
         * （cwd 是 `<根>\server` 时再往上一层）。**永远返回绝对路径**。
         */
        private fun resolveProjectRoot(storage: Path): Path {
            System.getenv("COMFYHUB_PROJECT_ROOT")
                ?.takeIf { it.isNotBlank() }
                ?.let { return Paths.get(it).toAbsolutePath().normalize() }

            if (storage.fileName?.toString().equals("storage", ignoreCase = true)) {
                storage.parent?.let { return it.toAbsolutePath().normalize() }
            }
            val cwd = Paths.get("").toAbsolutePath().normalize()
            return if (cwd.fileName?.toString().equals("server", ignoreCase = true) && cwd.parent != null) {
                cwd.parent
            } else {
                cwd
            }
        }

        /**
         * 默认的数据库 URL（P2 起是 **SQLite**）。
         *
         * 位置：`<项目根>\.db\comfyhub.db` —— 与老的 `.mysql\` 同一个约定（点开头、在根目录下、
         * 属于"可写运行数据"，打包脚本的清理清单不许动它）。同目录还会有 SQLite 自己产生的
         * `-wal` / `-shm`，**一起拷才算一份完整备份**。
         *
         * `COMFYHUB_JDBC_URL` 显式给值就原样用它 —— 搬迁时就是靠它指向老 MySQL 库
         * （`jdbc:mysql://127.0.0.1:3307/comfy_hub?...`）。路径统一成正斜杠：
         * xerial 在 Windows 上认 `D:/x/y.db` 这种写法，反斜杠形式的容错没保证。
         */
        internal fun defaultJdbcUrl(projectRoot: Path): String {
            val explicit = System.getenv("COMFYHUB_JDBC_URL")?.takeIf { it.isNotBlank() }
            if (explicit != null) return explicit
            val file = projectRoot.resolve(".db").resolve("comfyhub.db").toAbsolutePath().normalize()
            return "jdbc:sqlite:" + file.toString().replace('\\', '/')
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
        append("数据库=${if (dbDialect == DbDialect.SQLITE) "SQLite" else "MySQL（仅搬迁用）"}\n")
        append("jdbcUrl=$jdbcUrl\n")
        if (dbDialect == DbDialect.MYSQL) append("dbUser=$dbUser\n")
        append("storage=$storageDir\n")
        append("maxUploadMB=${maxUploadBytes / 1024 / 1024}\n")
        append("projectRoot=$projectRoot\n")
        append("comfyUrl=$comfyUrl\n")
        append("comfyOutput=${comfyOutputDir ?: "(未设置，走 HTTP 下载)"}\n")
    }
}
