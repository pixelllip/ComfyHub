package com.comfyhub.ai.tools

import kotlinx.serialization.Serializable
import org.slf4j.LoggerFactory
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.security.MessageDigest
import java.time.Instant

/**
 * Skills 仓库（M5 / AIH-037~045）。
 *
 * **磁盘是正文的真源**，数据库只当索引用 —— 所以 AI 说"我要注册这个 skill"、
 * 用户从右侧栏删掉一个，都是立刻落盘 / 立刻消失，**不需要重启 App**；
 * 下一次 Run 渲染系统提示词时读到的就是最新目录（用户说的"在新对话生效"）。
 *
 * 布局（只扫根目录下一层，不做递归 glob 搜索）：
 *
 * ```
 * <内置根>/<name>/SKILL.md        项目自带，只读
 * <用户根>/<name>/SKILL.md        AI 注册 / 导入进来的，可删可改
 * <用户根>/<name>.md              平铺写法也认
 * ```
 *
 * frontmatter 是 YAML 的最小子集（`key: value`），字段：
 * `name`（必需，kebab-case，且必须与目录名一致）、`description`（必需）、
 * `whenToUse` / `version` / `user-invocable` / `disable-model-invocation`（可选）。
 *
 * 安全：非法项**不静默忽略** —— 照样列出来，但带 [SkillDto.validationError]，
 * 并且不进系统提示、不能被 load_skill 加载（AIH-038）。
 */
@Serializable
data class SkillDto(
    val name: String,
    val description: String = "",
    val whenToUse: String? = null,
    val version: String? = null,
    /** builtin / user */
    val source: String = "user",
    val enabled: Boolean = true,
    val userInvocable: Boolean = true,
    val modelInvocable: Boolean = true,
    /** 正文 sha256 前 16 位：界面显示 + 变更审计（AIH-047 的 digest） */
    val digest: String? = null,
    val sizeBytes: Long = 0,
    val fileCount: Int = 1,
    val updatedAt: String? = null,
    /** 非空表示这个 skill 不合法：列表里显示诊断，且不参与对话 */
    val validationError: String? = null,
    /** 非空的"提示"类信息（例如覆盖了同名内置 skill）：不影响可用性 */
    val conflict: String? = null,
    /** 来源说明（例如 `dsh:%USERPROFILE%\.dsh\skills`） */
    val origin: String? = null,
)

@Serializable
data class SkillImportResult(
    val imported: Int = 0,
    val skipped: Int = 0,
    val source: String,
    val errors: List<String> = emptyList(),
    val skills: List<SkillDto> = emptyList(),
)

class SkillStore(
    /** 项目自带（只读）：`<根>\skills\builtin` */
    val builtinRoot: Path,
    /** 用户 / AI 注册（可写）：`<storage>\ai\skills` */
    val userRoot: Path,
) {
    private val log = LoggerFactory.getLogger(SkillStore::class.java)

    companion object {
        /** 名称规则：kebab-case，最长 64（AIH-038） */
        private val NAME_RE = Regex("^[a-z0-9][a-z0-9-]{0,63}$")

        /**
         * description 的**安全上限**（只约束 `save()` 这条写入路径）。
         *
         * 扫描已有 skill 时**不因为描述长就把它判成非法** —— DSH 里就有 700+ 字的描述
         * （`anima-nsfw-prompt`），那是正常内容，不是错误。注入系统提示时会截断到 240 字，
         * 所以长度只影响"给模型看多少"，不该影响"能不能用"。
         */
        const val MAX_DESCRIPTION = 4000
        const val MAX_BODY_BYTES = 256 * 1024
        const val MAX_IMPORT_FILES = 300
        const val MAX_IMPORT_BYTES = 8L * 1024 * 1024

        /** DSH 的 skills 目录：**只在用户显式点"导入"时读一次**，运行时绝不依赖它。 */
        fun dshRoot(): Path? {
            val home = System.getenv("USERPROFILE") ?: System.getProperty("user.home") ?: return null
            val dir = Path.of(home, ".dsh", "skills")
            return if (Files.isDirectory(dir)) dir else null
        }

        fun validateName(name: String) {
            if (!NAME_RE.matches(name)) {
                throw ToolFailure(
                    "INVALID_SKILL_NAME",
                    "非法 skill 名「$name」：只允许小写字母 / 数字 / 连字符，且以字母或数字开头（≤64 字符）"
                )
            }
        }

        /** 极简 frontmatter 解析：只认 `key: value`，带引号的去引号，`#` 起注释。 */
        fun parseFrontMatter(raw: String): Pair<Map<String, String>, String> {
            val text = raw.removePrefix("\uFEFF")
            val lines = text.lines()
            if (lines.firstOrNull()?.trim() != "---") return emptyMap<String, String>() to text

            val meta = LinkedHashMap<String, String>()
            var bodyStart = -1
            for (i in 1 until lines.size) {
                val line = lines[i].trim()
                if (line == "---") {
                    bodyStart = i + 1
                    break
                }
                if (line.isEmpty() || line.startsWith("#")) continue
                val idx = line.indexOf(':')
                if (idx <= 0) continue
                val key = line.substring(0, idx).trim().lowercase().replace('_', '-')
                var value = line.substring(idx + 1).trim()
                if (!value.startsWith("\"") && !value.startsWith("'")) {
                    value = value.substringBefore(" #").trim()
                }
                value = value.trim().trim('"').trim('\'')
                meta[key] = value
            }
            if (bodyStart < 0) return emptyMap<String, String>() to text
            return meta to lines.drop(bodyStart).joinToString("\n")
        }

        fun sha256(text: String): String {
            val digest = MessageDigest.getInstance("SHA-256").digest(text.toByteArray(StandardCharsets.UTF_8))
            return digest.joinToString("") { "%02x".format(it) }.take(16)
        }
    }

    // -----------------------------------------------------------------------
    //  扫描 / 读取
    // -----------------------------------------------------------------------

    /**
     * 全量扫描（内置 + 用户）。
     *
     * 同名冲突按优先级解决（用户 200 > 内置 100，AIH-037）：**用户版本胜出并保持可用**，
     * 内置那份不再出现，胜出者带上 [SkillDto.conflict] 供界面显示 ——
     * 冲突是提示，不是把功能关掉的理由。
     */
    fun scan(): List<SkillDto> {
        val builtin = scanRoot(builtinRoot, "builtin", null).associateBy { it.name }
        val user = scanRoot(userRoot, "user", null).associateBy { it.name }
        val out = mutableListOf<SkillDto>()
        builtin.forEach { (name, dto) ->
            val override = user[name]
            out += if (override != null) {
                override.copy(conflict = override.conflict ?: "覆盖了同名内置 skill")
            } else {
                dto
            }
        }
        user.forEach { (name, dto) -> if (name !in builtin) out += dto }
        return out.sortedBy { it.name }
    }

    /** 可以进系统提示 / 能被加载的 skill。 */
    fun catalog(): List<SkillDto> = scan().filter { it.validationError == null && it.enabled }

    fun find(name: String): SkillDto? = scan().firstOrNull { it.name == name }

    /** 读取正文（**不缓存**：AI 刚注册完，下一次 load_skill 就要读到新的）。 */
    fun read(name: String): Pair<SkillDto, String>? {
        val dto = find(name) ?: return null
        if (dto.validationError != null) {
            throw ToolFailure("SKILL_INVALID", "skill「$name」不合法，不能加载：${dto.validationError}")
        }
        val file = fileOf(name) ?: throw ToolFailure("SKILL_NOT_FOUND", "找不到 skill「$name」的正文文件")
        val raw = Files.readString(file, StandardCharsets.UTF_8)
        val body = parseFrontMatter(raw).second.trim()
        return dto to body
    }

    /** 注册 / 覆盖一个 skill：写 `<用户根>/<name>/SKILL.md`。 */
    fun save(
        name: String,
        description: String,
        whenToUse: String? = null,
        content: String,
        origin: String? = null,
    ): SkillDto {
        validateName(name)
        val desc = description.trim()
        if (desc.isEmpty()) throw ToolFailure("INVALID_ARGUMENT", "description 必填：说明这个 skill 什么时候用")
        if (desc.length > MAX_DESCRIPTION) {
            throw ToolFailure("INVALID_ARGUMENT", "description 太长（${desc.length} > $MAX_DESCRIPTION 字符）")
        }
        val body = content.trim()
        if (body.isEmpty()) throw ToolFailure("INVALID_ARGUMENT", "skill 正文不能为空")
        val bytes = body.toByteArray(StandardCharsets.UTF_8).size
        if (bytes > MAX_BODY_BYTES) {
            throw ToolFailure("INVALID_ARGUMENT", "skill 正文太大（$bytes > $MAX_BODY_BYTES 字节）")
        }

        Files.createDirectories(userRoot)
        val dir = safeChild(userRoot, name)
        Files.createDirectories(dir)
        val file = dir.resolve("SKILL.md")
        Files.writeString(file, render(name, desc, whenToUse, body), StandardCharsets.UTF_8)
        log.info("skill 已注册: {}（{} 字节，来源 {}）", name, bytes, origin ?: "ai")
        return find(name) ?: throw ToolFailure("IO_ERROR", "写入后读不到 skill「$name」")
    }

    private fun render(name: String, description: String, whenToUse: String?, body: String): String = buildString {
        append("---\n")
        append("name: ").append(name).append('\n')
        append("description: ").append(description.replace('\n', ' ')).append('\n')
        if (!whenToUse.isNullOrBlank()) append("whenToUse: ").append(whenToUse.replace('\n', ' ')).append('\n')
        append("version: 1\n")
        append("---\n\n")
        append(body.trim()).append('\n')
    }

    /** 删除：**只允许删用户来源**（内置受保护，AIH-037 的优先级规则）。 */
    fun delete(name: String): Boolean {
        validateName(name)
        val dto = find(name) ?: return false
        if (dto.source != "user") {
            throw ToolFailure("SKILL_READONLY", "「$name」是内置 skill，只能停用不能删除")
        }
        val file = fileOf(name) ?: return false
        runCatching {
            val parent = file.parent
            Files.deleteIfExists(file)
            // bundle 目录：整个删掉（含 references/），平铺 .md 只删文件
            if (parent != null && parent != userRoot && parent.startsWith(userRoot)) {
                Files.walk(parent).use { stream ->
                    stream.sorted(java.util.Comparator.reverseOrder<Path>())
                        .forEach { p -> runCatching { Files.deleteIfExists(p) } }
                }
            }
        }.onFailure { log.warn("删除 skill {} 失败: {}", name, it.message) }
        log.info("skill 已删除: {}", name)
        return true
    }

    // -----------------------------------------------------------------------
    //  导入（用户显式动作；运行时绝不自动读 .dsh）
    // -----------------------------------------------------------------------

    /**
     * 从某个目录批量导入（典型来源：`%USERPROFILE%\.dsh\skills`）。
     *
     * 安全约束（AIH-044 的精神，先校验再复制）：拒绝符号链接、拒绝路径穿越、
     * 限制单文件与总量、限制文件数；解不开的条目记进 [SkillImportResult.errors]。
     */
    fun importFrom(sourceDir: Path, originLabel: String = sourceDir.toString()): SkillImportResult {
        if (!Files.isDirectory(sourceDir)) {
            throw ToolFailure("INVALID_ARGUMENT", "不是目录：$sourceDir")
        }
        var imported = 0
        var skipped = 0
        val errors = mutableListOf<String>()
        val importedSkills = mutableListOf<SkillDto>()

        val entries = Files.list(sourceDir).use { it.toList() }
        entries.sortedBy { it.fileName.toString() }.forEach { entry ->
            val label = entry.fileName.toString()
            try {
                if (Files.isSymbolicLink(entry)) {
                    skipped++
                    errors += "$label：符号链接，已跳过"
                    return@forEach
                }
                val (skillFile, dir) = when {
                    Files.isDirectory(entry, LinkOption.NOFOLLOW_LINKS) -> {
                        val candidate = entry.resolve("SKILL.md")
                        if (Files.isRegularFile(candidate)) candidate to entry
                        else entry.resolve("skill.md").takeIf { Files.isRegularFile(it) }?.let { it to entry }
                            ?: run { skipped++; errors += "$label：没有 SKILL.md"; return@forEach }
                    }
                    label.lowercase().endsWith(".md") -> entry to null
                    else -> {
                        skipped++
                        return@forEach
                    }
                }
                val name = label.removeSuffix(".md").removeSuffix(".MD")
                validateName(name)

                if (dir != null) {
                    copyTree(dir, safeChild(userRoot, name), errors, label)
                } else {
                    Files.createDirectories(userRoot)
                    Files.copy(skillFile, safeChild(userRoot, "$name.md"), StandardCopyOption.REPLACE_EXISTING)
                }
                find(name)?.let {
                    importedSkills += it.copy(origin = originLabel)
                    if (it.validationError != null) errors += "$name：${it.validationError}"
                }
                imported++
            } catch (e: ToolFailure) {
                skipped++
                errors += "$label：${e.message}"
            } catch (e: Exception) {
                skipped++
                errors += "$label：${e.message}"
            }
        }
        log.info("skill 导入完成：来源 {} 导入 {} 跳过 {} 错误 {}", originLabel, imported, skipped, errors.size)
        return SkillImportResult(imported, skipped, originLabel, errors.take(50), importedSkills)
    }

    private fun copyTree(from: Path, to: Path, errors: MutableList<String>, label: String) {
        var files = 0
        var bytes = 0L
        Files.walk(from, 4).use { stream ->
            stream.forEach { src ->
                val rel = from.relativize(src)
                if (rel.any { it.toString() == ".." }) return@forEach
                if (Files.isSymbolicLink(src)) return@forEach
                if (Files.isDirectory(src)) {
                    Files.createDirectories(to.resolve(rel))
                    return@forEach
                }
                val size = runCatching { Files.size(src) }.getOrDefault(0L)
                files++
                bytes += size
                if (files > MAX_IMPORT_FILES || bytes > MAX_IMPORT_BYTES) {
                    throw ToolFailure("IMPORT_TOO_LARGE", "$label：条目太多或体积过大，已中止（保护性限制）")
                }
                Files.copy(src, to.resolve(rel), StandardCopyOption.REPLACE_EXISTING)
            }
        }
    }

    // -----------------------------------------------------------------------

    private fun safeChild(root: Path, name: String): Path {
        val child = root.resolve(name).normalize()
        if (!child.startsWith(root.normalize())) {
            throw ToolFailure("PATH_DENIED", "非法 skill 路径：$name")
        }
        return child
    }

    /** bundle 优先，其次平铺 `<name>.md`。 */
    private fun fileOf(name: String): Path? {
        val bundle = userRoot.resolve(name).resolve("SKILL.md")
        if (Files.isRegularFile(bundle)) return bundle
        val flat = userRoot.resolve("$name.md")
        if (Files.isRegularFile(flat)) return flat
        val builtinBundle = builtinRoot.resolve(name).resolve("SKILL.md")
        if (Files.isRegularFile(builtinBundle)) return builtinBundle
        val builtinFlat = builtinRoot.resolve("$name.md")
        if (Files.isRegularFile(builtinFlat)) return builtinFlat
        return null
    }

    private fun scanRoot(root: Path, source: String, origin: String?): List<SkillDto> {
        if (!Files.isDirectory(root)) return emptyList()
        // 同名时 **bundle（<name>/SKILL.md）胜出**：与 [fileOf] 的查找顺序保持一致，
        // 否则"平铺 <name>.md + 目录 <name>/"同时存在时，哪份生效取决于文件系统的枚举顺序。
        val byName = LinkedHashMap<String, Pair<SkillDto, Boolean>>()
        runCatching {
            Files.list(root).use { stream ->
                stream.forEach { entry ->
                    val fileName = entry.fileName.toString()
                    val isDir = Files.isDirectory(entry, LinkOption.NOFOLLOW_LINKS)
                    val file = when {
                        isDir -> entry.resolve("SKILL.md").takeIf { Files.isRegularFile(it) }
                            ?: entry.resolve("skill.md").takeIf { Files.isRegularFile(it) }
                        fileName.lowercase().endsWith(".md") -> entry
                        else -> null
                    } ?: return@forEach
                    runCatching { parseOne(file, fileName.removeSuffix(".md").removeSuffix(".MD"), source, origin) }
                        .onSuccess { dto ->
                            val existing = byName[dto.name]
                            if (existing == null || (isDir && !existing.second)) {
                                byName[dto.name] = dto to isDir
                            }
                        }
                        .onFailure { log.warn("解析 skill {} 失败: {}", file, it.message) }
                }
            }
        }.onFailure { log.warn("扫描 skill 目录 {} 失败: {}", root, it.message) }
        return byName.values.map { it.first }
    }

    private fun parseOne(file: Path, fallbackName: String, source: String, origin: String?): SkillDto {
        val raw = Files.readString(file, StandardCharsets.UTF_8)
        val (meta, body) = parseFrontMatter(raw)
        val name = meta["name"]?.takeIf { it.isNotBlank() } ?: fallbackName
        val fileCount = runCatching {
            val parent = file.parent
            if (parent != null && parent.fileName?.toString() == name) {
                Files.walk(parent, 3).use { it.filter { p -> Files.isRegularFile(p) }.count().toInt() }
            } else 1
        }.getOrDefault(1)
        val updatedAt = runCatching { Files.getLastModifiedTime(file).toInstant().toString() }.getOrNull()
            ?: Instant.now().toString()

        var error: String? = null
        fun fail(msg: String) {
            if (error == null) error = msg
        }
        if (!NAME_RE.matches(name)) fail("name 必须是 kebab-case（小写字母 / 数字 / 连字符，≤64）")
        if (name != fallbackName && file.parent?.fileName?.toString() == fallbackName) {
            fail("frontmatter 的 name「$name」与目录名「$fallbackName」不一致")
        }
        val description = meta["description"].orEmpty()
        if (description.isBlank()) fail("缺少 description")
        if (body.isBlank()) fail("正文为空")
        if (body.toByteArray(StandardCharsets.UTF_8).size > MAX_BODY_BYTES) fail("正文超过 $MAX_BODY_BYTES 字节")

        val userInvocable = meta["user-invocable"]?.toBooleanStrictOrNull() ?: true
        val modelInvocable = (meta["disable-model-invocation"]?.toBooleanStrictOrNull() ?: false).not()

        return SkillDto(
            name = name,
            description = description,
            whenToUse = meta["whentouse"]?.takeIf { it.isNotBlank() },
            version = meta["version"]?.takeIf { it.isNotBlank() },
            source = source,
            enabled = true,
            userInvocable = userInvocable,
            modelInvocable = modelInvocable,
            digest = sha256(raw),
            sizeBytes = raw.toByteArray(StandardCharsets.UTF_8).size.toLong(),
            fileCount = fileCount,
            updatedAt = updatedAt,
            validationError = error,
            origin = origin,
        )
    }
}
