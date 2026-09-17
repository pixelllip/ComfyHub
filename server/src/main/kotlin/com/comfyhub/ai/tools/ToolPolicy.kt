package com.comfyhub.ai.tools

import com.comfyhub.AppJson
import com.comfyhub.SettingsRepo
import kotlinx.serialization.Serializable
import org.slf4j.LoggerFactory
import java.nio.file.Files
import java.nio.file.InvalidPathException
import java.nio.file.Path
import java.nio.file.Paths

/**
 * 工具权限策略（用户要求：「默认不能修改 comfy 目录以外的内容」）。
 *
 * 三条硬规矩：
 *
 *  1. **写操作只允许落在 [ToolPolicyConfig.writeRoots] 里**，出厂默认只有 `<根>\comfyui`。
 *     往别处写 → `deny`（不是"问一下"，问一下等于给了个容易被说服的入口）。
 *  2. **读操作只允许落在 [ToolPolicyConfig.readRoots] 里**，出厂默认 `<根>\comfyui` 与
 *     `<根>\storage`（产物目录只读）。
 *  3. **运行期数据永远不可写**：`.git` / `.mysql` / `.run` / `node_modules` 即使被用户加进
 *     writeRoots 也拒绝 —— 它们是本项目的"内脏"，模型碰坏一个就等于把应用弄坏。
 *
 * 路径判定一律走**真实路径**（`toRealPath`）：符号链接 / `..` / 短路径名都不能绕过。
 * 目标还不存在时，取**最近的存在祖先**做真实路径，再拼回剩余部分。
 *
 * 策略可被用户覆盖并落库（`app_settings` 的 `ai.tools.policy`），
 * [ToolPolicyConfig.overrides] 记每个工具的权限档；用户没覆盖的工具用出厂档。
 */
@Serializable
data class ToolPolicyConfig(
    /** 允许写入的目录（绝对路径或相对项目根）；空 = 出厂默认（comfyui） */
    val writeRoots: List<String> = emptyList(),
    /** 允许读取的目录；空 = 出厂默认（comfyui + storage） */
    val readRoots: List<String> = emptyList(),
    /** 逐工具权限覆盖：工具名 → allow/ask/deny */
    val overrides: Map<String, String> = emptyMap(),
    /**
     * 权限档（用户建议 ⑤）：`ask` = 默认，需要审批的工具得等用户点批准；
     * `full` = **完全权限**，AI 不必再等批准（在 AI 工作台输入区里切换）。
     *
     * 它只决定"要不要问一下"，**不放宽路径白名单**：完全权限下越界写照样被拒，
     * 被显式 `deny` 的工具也照样不放宽（deny 的含义是"这个工具不该用它"）。
     */
    val permissionMode: String = PERMISSION_ASK,
    /** 单次回复最多工具轮数（AIH-036 建议 8） */
    val maxToolSteps: Int = 8,
    /** 单个 Run 最多工具调用次数 */
    val maxCallsPerRun: Int = 16,
    /**
     * 单个 Run 最多**主动查询 ComfyUI** 的次数（AIH-036：一次回复最多查 3 次）。
     *
     * 和 [maxCallsPerRun] 是两件事：后者是总预算，这个是"不许拿 ComfyUI 当轮询器"。
     */
    val maxComfyQueriesPerRun: Int = 3,
    /** 一次工具最多读取的字节数 */
    val maxReadBytes: Int = 256 * 1024,
    /** 一次 write_file 最多写入的字节数 */
    val maxWriteBytes: Int = 1024 * 1024,
) {
    companion object {
        const val KEY = "ai.tools.policy"
        const val DEFAULT_WRITE_DIR = "comfyui"
        val DEFAULT_READ_DIRS = listOf("comfyui", "storage")

        /** 权限档：默认「询问」/「完全权限」 */
        const val PERMISSION_ASK = "ask"
        const val PERMISSION_FULL = "full"
        val PERMISSION_MODES = setOf(PERMISSION_ASK, PERMISSION_FULL)

        /** 永远不可写：本项目的运行期数据、源码控制与依赖目录 */
        val FORBIDDEN_SEGMENTS = setOf(".git", ".mysql", ".run", "node_modules")
    }
}

class ToolPolicy(
    val projectRoot: Path,
    val config: ToolPolicyConfig,
) {
    val writeRoots: List<Path> = expand(config.writeRoots.ifEmpty { listOf(ToolPolicyConfig.DEFAULT_WRITE_DIR) })
    val readRoots: List<Path> = expand(config.readRoots.ifEmpty { ToolPolicyConfig.DEFAULT_READ_DIRS })

    private fun expand(entries: List<String>): List<Path> = entries.mapNotNull { raw ->
        runCatching {
            val p = Paths.get(raw.trim())
            (if (p.isAbsolute) p else projectRoot.resolve(p)).normalize().toAbsolutePath()
        }.onFailure { LOG.warn("忽略非法路径配置 {}: {}", raw, it.message) }.getOrNull()
    }

    /**
     * 生效权限：用户覆盖 → 出厂档 → **再按权限档放宽**。
     *
     * 「完全权限」只把 `ask` 变成 `allow`：
     *  - `deny` 不动（那是"禁用这个工具"，不是"要不要问一下"）；
     *  - 路径白名单不动（在 [resolveWrite] 里另判，越界照样拒绝）。
     * 配置里写了非法档位时**一律回落「询问」**：权限这种事不能因为配置坏了就放开。
     */
    fun accessFor(tool: AgentTool): ToolAccess {
        val base = ToolAccess.parse(config.overrides[tool.name]) ?: tool.defaultAccess
        return if (fullPermission && base == ToolAccess.ASK) ToolAccess.ALLOW else base
    }

    /** 用户选的权限档（非法值回落 ask）。 */
    val permissionMode: String =
        config.permissionMode.takeIf { it in ToolPolicyConfig.PERMISSION_MODES }
            ?: ToolPolicyConfig.PERMISSION_ASK

    /** 完全权限：AI 不必再等批准。 */
    val fullPermission: Boolean get() = permissionMode == ToolPolicyConfig.PERMISSION_FULL

    /** 用户显式覆盖过档位（用于界面上的"已覆盖"标记）。 */
    fun isOverridden(tool: AgentTool): Boolean = ToolAccess.parse(config.overrides[tool.name]) != null

    /** 读路径：必须落在 readRoots 内。 */
    fun resolveRead(raw: String): Path = resolve(raw, readRoots, "读")

    /** 写路径：必须落在 writeRoots 内，且不能落在禁写段。 */
    fun resolveWrite(raw: String): Path = resolve(raw, writeRoots, "写")

    private fun resolve(raw: String, roots: List<Path>, verb: String): Path {
        val text = raw.trim()
        if (text.isEmpty()) throw ToolFailure("INVALID_ARGUMENT", "路径不能为空")
        if (text.contains('\u0000')) throw ToolFailure("PATH_DENIED", "路径包含非法字符")

        val candidate = try {
            val p = Paths.get(text)
            (if (p.isAbsolute) p else projectRoot.resolve(p)).normalize().toAbsolutePath()
        } catch (e: InvalidPathException) {
            throw ToolFailure("PATH_DENIED", "路径无法解析：${e.message}")
        }

        rejectForbidden(candidate, verb)

        val real = realPath(candidate)
            ?: throw ToolFailure("PATH_DENIED", "$verb 路径不可解析：$text")

        val hit = roots.firstOrNull { real.startsWith(it) }
            ?: throw ToolFailure(
                "PATH_DENIED",
                "拒绝$verb $text：只允许在 ${roots.joinToString("、") { it.toString() }} 之内" +
                    "（可在「设置 → AI 工具权限」里调整）"
            )

        if (real == hit && verb == "写") {
            throw ToolFailure("PATH_DENIED", "拒绝把目录根本身当作写入目标：$text")
        }
        return real
    }

    private fun rejectForbidden(path: Path, verb: String) {
        val relative = runCatching { projectRoot.relativize(path) }.getOrNull() ?: return
        val segments = relative.map { it.toString().lowercase() }
        val hit = segments.firstOrNull { it in ToolPolicyConfig.FORBIDDEN_SEGMENTS } ?: return
        throw ToolFailure(
            "PATH_DENIED",
            "拒绝$verb ${path.fileName}：$hit 是本项目的运行期数据 / 依赖目录，任何工具都不可改"
        )
    }

    /**
     * 真实路径：存在就直接 `toRealPath`（解掉符号链接）；
     * 不存在就沿父目录往上找到第一个存在的祖先，再拼回剩余部分。
     */
    private fun realPath(path: Path): Path? {
        if (Files.exists(path)) {
            return runCatching { path.toRealPath() }.getOrNull()
        }
        var cursor: Path? = path
        val tail = ArrayDeque<String>()
        while (cursor != null && !Files.exists(cursor)) {
            val name = cursor.fileName?.toString() ?: return null
            tail.addFirst(name)
            cursor = cursor.parent
        }
        var base = cursor?.let { runCatching { it.toRealPath() }.getOrNull() } ?: return null
        tail.forEach { base = base.resolve(it) }
        return base.normalize()
    }

    companion object {
        private val LOG = LoggerFactory.getLogger(ToolPolicy::class.java)

        /** 从数据库读策略（读失败 → 出厂默认，绝不因为设置坏了就放开权限）。 */
        fun load(projectRoot: Path): ToolPolicy {
            val raw = runCatching { SettingsRepo.get(ToolPolicyConfig.KEY) }.getOrNull()
            val config = raw?.let {
                runCatching { AppJson.decodeFromString(ToolPolicyConfig.serializer(), it) }
                    .onFailure { e -> LOG.warn("解析 {} 失败，回退出厂策略: {}", ToolPolicyConfig.KEY, e.message) }
                    .getOrNull()
            } ?: ToolPolicyConfig()
            return ToolPolicy(projectRoot, config)
        }

        fun save(config: ToolPolicyConfig) {
            SettingsRepo.put(ToolPolicyConfig.KEY, AppJson.encodeToString(ToolPolicyConfig.serializer(), config))
        }
    }
}
