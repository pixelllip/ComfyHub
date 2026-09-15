package com.comfyhub.ai

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import org.slf4j.LoggerFactory
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.nio.file.attribute.PosixFilePermission
import java.util.Base64

/**
 * 凭据服务（AIH-012 / AIH-013 / AIH-015）。
 *
 * 语义完全照 DSH：对外只有 `set / describe / unset`，`resolve` **只允许后端
 * Provider 请求内部调用**，任何 DTO 都不含密钥值。
 *
 * 存储优先级：
 *  1. 进程环境变量（只读，最高优先级；适合 CI / 服务器）
 *  2. ComfyHub 受管凭据库：**Windows DPAPI(CurrentUser) 加密**后落盘
 *
 * 关键约束：DPAPI 不可用时 `set()` 直接失败，**绝不退化成明文落盘** ——
 * 否则就是"把明文包装成加密存储"，正是需求里明确禁止的（AIH-015）。
 */
class CredentialService(
    private val dataDir: Path,
    private val env: Map<String, String> = System.getenv(),
) {
    private val log = LoggerFactory.getLogger(CredentialService::class.java)
    private val file: Path = dataDir.resolve("credentials.dpapi.json")
    private val json = Json { prettyPrint = true; ignoreUnknownKeys = true }

    companion object {
        /** 值里不允许出现换行/控制字符：它会被塞进 HTTP Header。 */
        private val ALLOWED_VALUE = Regex("^[\\x21-\\x7E]+$")
        private val ENV_PREFIX = "COMFYHUB_CRED_"
    }

    // -----------------------------------------------------------------------
    //  对外接口（不含任何回读值的方法给路由层用）
    // -----------------------------------------------------------------------

    fun describe(ref: String?): CredentialStatusDto {
        if (ref.isNullOrBlank()) return CredentialStatusDto(configured = false, source = "none", writable = false)
        val envValue = envValue(ref)
        if (envValue != null) {
            return CredentialStatusDto(configured = envValue.isNotEmpty(), source = "env", writable = false)
        }
        val stored = readStore()[ref] != null
        return CredentialStatusDto(configured = stored, source = if (stored) "managed" else "none", writable = true)
    }

    fun set(ref: String, rawValue: String) {
        val value = sanitize(rawValue)
        val sealed = runCatching { DpapiSecretBox.protect(value) }.getOrElse { e ->
            throw AiException(
                AiErrorCode.CONFIG_ERROR,
                "无法加密保存密钥（Windows DPAPI 不可用）：${e.message}"
            )
        }
        val store = readStore().toMutableMap()
        store[ref] = sealed
        writeStore(store)
        log.info("已保存凭据 {}（DPAPI 加密，不记录值）", ref)
    }

    fun unset(ref: String): Boolean {
        val store = readStore().toMutableMap()
        val removed = store.remove(ref) != null
        if (removed) writeStore(store)
        if (removed) log.info("已移除凭据 {}", ref)
        return removed
    }

    /** **仅后端内部使用**：解析出明文用于构造上游请求头。 */
    fun resolve(ref: String?): String? {
        if (ref.isNullOrBlank()) return null
        envValue(ref)?.let { return it.ifEmpty { null } }
        val sealed = readStore()[ref] ?: return null
        return runCatching { DpapiSecretBox.unprotect(sealed) }.getOrElse { e ->
            log.error("凭据 {} 解密失败（可能换过 Windows 用户或凭据文件被改写）", ref)
            throw AiException(AiErrorCode.MISSING_CREDENTIAL, "凭据解密失败，请在设置中重新填写")
        }
    }

    // -----------------------------------------------------------------------
    //  输入校验（AIH-012：粘贴 NAME=value 要报错，而不是当成密钥存下来）
    // -----------------------------------------------------------------------

    fun sanitize(rawValue: String): String {
        if (rawValue.isBlank()) {
            throw AiException(AiErrorCode.CONFIG_ERROR, "密钥不能为空；留空表示不修改已有密钥")
        }
        var v = rawValue.trim()
        if ((v.startsWith("\"") && v.endsWith("\"")) || (v.startsWith("'") && v.endsWith("'"))) {
            throw AiException(AiErrorCode.CONFIG_ERROR, "请只粘贴密钥本身，不要带引号")
        }
        if (v.contains('=') && v.substringBefore('=').matches(Regex("^[A-Za-z_][A-Za-z0-9_]*$"))) {
            throw AiException(AiErrorCode.CONFIG_ERROR, "请只粘贴密钥的值，不要粘贴 NAME=value 这种整行")
        }
        if (!ALLOWED_VALUE.matches(v)) {
            throw AiException(AiErrorCode.CONFIG_ERROR, "密钥只能包含可打印字符，且不能有空格或换行")
        }
        return v
    }

    // -----------------------------------------------------------------------
    //  内部
    // -----------------------------------------------------------------------

    private fun envValue(ref: String): String? {
        env[ref]?.let { return it }
        env[ENV_PREFIX + ref]?.let { return it }
        return null
    }

    private fun readStore(): Map<String, String> {
        if (!Files.isRegularFile(file)) return emptyMap()
        return runCatching {
            val root = json.parseToJsonElement(Files.readString(file, StandardCharsets.UTF_8))
            (root as? JsonObject)?.mapNotNull { (k, v) ->
                (v as? JsonPrimitive)?.contentOrNull?.let { k to it }
            }?.toMap() ?: emptyMap()
        }.getOrElse {
            log.error("凭据库损坏，按空处理（不影响启动）: {}", it.message)
            emptyMap()
        }
    }

    private fun writeStore(store: Map<String, String>) {
        Files.createDirectories(dataDir)
        val root = buildJsonObject { store.forEach { (k, v) -> put(k, JsonPrimitive(v)) } }
        val tmp = file.resolveSibling(file.fileName.toString() + ".tmp")
        Files.writeString(tmp, json.encodeToString(JsonObject.serializer(), root), StandardCharsets.UTF_8)
        restrictPermissions(tmp)
        Files.move(tmp, file, StandardCopyOption.REPLACE_EXISTING)
    }

    /** 双保险：DPAPI 之外再收紧文件属主权限。 */
    private fun restrictPermissions(path: Path) {
        runCatching {
            val view = Files.getFileAttributeView(path, java.nio.file.attribute.PosixFileAttributeView::class.java)
            view?.setPermissions(
                setOf(PosixFilePermission.OWNER_READ, PosixFilePermission.OWNER_WRITE)
            )
        }
    }
}

/**
 * Windows DPAPI(CurrentUser) 封装。
 *
 * JVM 没有 DPAPI 绑定，这里通过 `powershell.exe -EncodedCommand` 调
 * `System.Security.Cryptography.ProtectedData`：载荷走子进程环境变量，
 * 结果走 stdout 的 base64，没有任何命令行拼串，因此不存在引号/转义注入。
 * 子进程继承后端自身的（隐藏）控制台，不会弹新窗口。
 */
object DpapiSecretBox {
    private val log = LoggerFactory.getLogger(DpapiSecretBox::class.java)

    private val protectScript = """
        ${'$'}ErrorActionPreference='Stop'
        Add-Type -AssemblyName System.Security
        ${'$'}b=[Convert]::FromBase64String(${'$'}env:CH_PAYLOAD)
        ${'$'}e=[System.Security.Cryptography.ProtectedData]::Protect(${'$'}b,${'$'}null,[System.Security.Cryptography.DataProtectionScope]::CurrentUser)
        [Console]::Out.Write([Convert]::ToBase64String(${'$'}e))
    """.trimIndent()

    private val unprotectScript = """
        ${'$'}ErrorActionPreference='Stop'
        Add-Type -AssemblyName System.Security
        ${'$'}b=[Convert]::FromBase64String(${'$'}env:CH_PAYLOAD)
        ${'$'}e=[System.Security.Cryptography.ProtectedData]::Unprotect(${'$'}b,${'$'}null,[System.Security.Cryptography.DataProtectionScope]::CurrentUser)
        [Console]::Out.Write([Convert]::ToBase64String(${'$'}e))
    """.trimIndent()

    fun protect(plain: String): String =
        run(protectScript, Base64.getEncoder().encodeToString(plain.toByteArray(StandardCharsets.UTF_8)))

    fun unprotect(sealedBase64: String): String {
        val out = run(unprotectScript, sealedBase64)
        return String(Base64.getDecoder().decode(out), StandardCharsets.UTF_8)
    }

    private fun run(script: String, payloadBase64: String): String {
        if (!isWindows()) error("DPAPI 仅在 Windows 上可用")
        val exe = powershellPath()
        val encoded = Base64.getEncoder()
            .encodeToString(script.toByteArray(StandardCharsets.UTF_16LE))
        val pb = ProcessBuilder(exe, "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded)
        pb.environment()["CH_PAYLOAD"] = payloadBase64
        pb.redirectErrorStream(false)
        val proc = pb.start()
        val stdout = proc.inputStream.readAllBytes().toString(StandardCharsets.UTF_8).trim()
        val stderr = proc.errorStream.readAllBytes().toString(StandardCharsets.UTF_8).trim()
        val code = proc.waitFor()
        if (code != 0 || stdout.isEmpty()) {
            log.debug("DPAPI 子进程失败 code={} err={}", code, stderr.take(400))
            error("DPAPI 调用失败（exit=$code）")
        }
        return stdout
    }

    fun isWindows(): Boolean =
        System.getProperty("os.name")?.lowercase()?.contains("win") == true

    private fun powershellPath(): String {
        val root = System.getenv("SystemRoot") ?: "C:\\Windows"
        val full = Path.of(root, "System32", "WindowsPowerShell", "v1.0", "powershell.exe")
        return if (Files.isRegularFile(full)) full.toString() else "powershell.exe"
    }
}
