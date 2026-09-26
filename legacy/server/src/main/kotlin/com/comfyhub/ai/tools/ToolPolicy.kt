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
     * `full` = **自动允许（无需批准）**，AI 不必再等批准（在 AI 工作台输入区里切换）。
     *
     * 它只决定"要不要问一下"，**不放宽路径白名单**：自动允许下越界写照样被拒，
     * 被显式 `deny` 的工具也照样不放宽（deny 的含义是"这个工具不该用它"）。
     */
    val permissionMode: String = PERMISSION_ASK,
    /** 单次回复最多工具轮数（AIH-036 建议 8 → 2026-09-22 上调到 16，理由见 [LEGACY_MAX_TOOL_STEPS]） */
    val maxToolSteps: Int = DEFAULT_MAX_TOOL_STEPS,
    /** 单个 Run 最多工具调用次数（可被"用户回答提问"重置，见 [ToolContext.resetBudget]） */
    val maxCallsPerRun: Int = DEFAULT_MAX_CALLS_PER_RUN,
    /**
     * 单个 Run 最多**主动查询 ComfyUI** 的次数（默认见 [DEFAULT_MAX_COMFY_QUERIES_PER_RUN]）。
     *
     * 和 [maxCallsPerRun] 是两件事：后者是总预算，这个是"不许拿 ComfyUI 当轮询器"。
     *
     * ⚠ 这一项**不是用户设置**（界面与 `ToolPolicyUpdate` 里都没有它），但 [ToolPolicy.save]
     * 会把整份配置写进 `app_settings`，于是老库里会冻住一份出厂值 —— 光改代码里的默认值不会生效。
     * 读库一律过 [normalizeStored]。
     */
    val maxComfyQueriesPerRun: Int = DEFAULT_MAX_COMFY_QUERIES_PER_RUN,
    /**
     * 单个 Run 最多**联网搜索**几次（默认见 [DEFAULT_MAX_WEB_QUERIES_PER_RUN]）。
     *
     * 与 [maxComfyQueriesPerRun] 同一个道理：那是"不许拿 ComfyUI 当轮询器"，
     * 这个是"不许拿搜索引擎当草稿纸"。⚠ 同样**不是用户设置**，所以必须进 [normalizeStored]，
     * 否则老库里会冻住一份出厂值。
     */
    val maxWebQueriesPerRun: Int = DEFAULT_MAX_WEB_QUERIES_PER_RUN,
    /** 单次 `read_file` 最多读取的字节数（2026-09-19 上调，见 [DEFAULT_MAX_READ_BYTES]） */
    val maxReadBytes: Int = DEFAULT_MAX_READ_BYTES,
    /** 一次 write_file 最多写入的字节数（2026-09-19 上调，见 [DEFAULT_MAX_WRITE_BYTES]） */
    val maxWriteBytes: Int = DEFAULT_MAX_WRITE_BYTES,
) {
    companion object {
        const val KEY = "ai.tools.policy"
        const val DEFAULT_WRITE_DIR = "comfyui"
        val DEFAULT_READ_DIRS = listOf("comfyui", "storage")

        /**
         * 权限档：默认「询问」/「自动允许（无需批准）」。
         *
         * 界面上这两个名字由 Dart 侧的 `AiToolPolicy.modeAskLabel` / `modeFullLabel` 给，
         * 系统提示里那一段与它们一字不差 —— 用户说"切到自动允许"时，模型看到的也是这个词。
         */
        const val PERMISSION_ASK = "ask"
        const val PERMISSION_FULL = "full"
        val PERMISSION_MODES = setOf(PERMISSION_ASK, PERMISSION_FULL)

        /** 永远不可写：本项目的运行期数据、源码控制与依赖目录 */
        val FORBIDDEN_SEGMENTS = setOf(".git", ".mysql", ".run", "node_modules")

        /**
         * 单次回复的工具轮数上限（**出厂值**，用户可在设置里改）。
         *
         * 8 → **16**（2026-09-22，用户建议 ③）：MCP 接进来之后"一轮完整的事"明显变长
         * —— 读工作流 → 查节点契约 → 查本机模型 → 摘分支 → 补线 → 提交，
         * 光这一步就是 6~8 个工具轮。2026-09-21 的三条真实对话都在第 8 轮被掐断，
         * 用户看到的是"已达到本次回复的工具轮数上限 8，再发一条消息它就能接着做"
         * （`bug-and-suggestion-9.22.md` 建议 ③ 的现场）。
         */
        const val DEFAULT_MAX_TOOL_STEPS = 16

        /** 旧出厂值（8 轮 / 16 次）：只用来识别"用户从没改过的库"，见 [bumpLegacyBudgets]。 */
        const val LEGACY_MAX_TOOL_STEPS = 8
        const val LEGACY_MAX_CALLS_PER_RUN = 16

        /**
         * 单个 Run 的工具调用次数上限（**出厂值**，用户可在设置里改）。
         *
         * 16 → **48**（同一条建议）：这一项是**可重置**的（用户每回答一次提问就归零，
         * 见 [ToolContext.resetBudget]），所以它的含义是"一次选择之后能做多少事"，
         * 而不是"整个 Run 只能做多少事"。绝对天花板另有其人
         * （`ToolRegistry.ABSOLUTE_CALL_CAP`），那条不受重置影响。
         */
        const val DEFAULT_MAX_CALLS_PER_RUN = 48

        /**
         * 出厂默认：一次回复里最多主动查 ComfyUI 几次。
         *
         * AIH-036 的原话是 3 次；2026-09-17 用户实测"3 次太少"（投递附件 + 查节点 + 查工作流
         * 很容易就撞上限），要求放宽到 9。**这是唯一的真源**：系统提示词里的那个数字由它插值，
         * 不许在别处再写一遍字面量。
         */
        const val DEFAULT_MAX_COMFY_QUERIES_PER_RUN = 9

        /**
         * 出厂默认：一次回复里最多联网搜索几次。
         *
         * 8 次的依据：一次典型的"查资料"是"搜 1~2 次 → 发现不对 → 换个说法再搜"，
         * 8 次足够走完两三轮；再往上就不是在用了，是在拿搜索引擎当草稿纸
         * （每一次都是真实的外部请求 + 一份要烧 token 的正文）。
         *
         * 另外免密钥额度是 1000 次/小时，8 次/轮 的设计让单机用户基本不可能撞上它。
         */
        const val DEFAULT_MAX_WEB_QUERIES_PER_RUN = 8

        /**
         * 单次读取上限：**2MB**（用户 2026-09-19：「单轮读写上限给得太低了，
         * 万一工作流本身有问题，AI 尝试修改工作流就会显得不够用」）。
         *
         * 以前是 256KB —— 一份界面格式的工作流 JSON（100+ 节点、带 `widgets_values`）
         * 轻轻松松就超过它，`read_file` 只能读到前面一截，模型自然改不动。
         * 现在 2MB 足够装下工作流本体；再往上（比如视频 / 模型文件）本来也不是它的活。
         */
        const val DEFAULT_MAX_READ_BYTES = 2 * 1024 * 1024

        /**
         * 单次写入上限：**4MB**（同一条用户建议）。
         *
         * 以前是 1MB：把一份大工作流改好写回去会直接 `TOO_LARGE`，用户看到的
         * 就是"AI 说它改不了"。写入仍然是白名单里的路径 + 有上限，**不是不限量**。
         */
        const val DEFAULT_MAX_WRITE_BYTES = 4 * 1024 * 1024

        /**
         * 从库里读出来的配置要过一遍这里：**内置预算一律以代码里的常量为准**。
         *
         * 为什么需要它：[ToolPolicy.save] 会把整份 [ToolPolicyConfig]（含默认值）写进
         * `app_settings`，所以老库里会冻着一份"当年的出厂值"。只改代码默认值的话，
         * 用户那边的库里还是旧数字，表现为"改了没用"（实测：3 → 9 时库里仍写着 3，
         * 且用户切一次权限档就会把当时的值写进去）。
         *
         * 所以**界面上不可改、纯粹是后端固定预算**的这几项都在这里强制回落到代码常量：
         * `maxComfyQueriesPerRun` / `maxReadBytes` / `maxWriteBytes`
         * （实测 2026-09-19：库里的 `ai.tools.policy` 冻着 `maxReadBytes:262144`，
         * 光走上调代码默认值这条路对老用户完全无效）。
         *
         * [maxWebQueriesPerRun] 同理：它是新加的固定预算，老库里根本没有这个键，
         * 但一旦用户切一次权限档，[save] 就会把当时的默认值写进去 —— 所以从一开始就归它管。
         *
         * [maxToolSteps] / [maxCallsPerRun] **不能**这样处理：它们是用户可改的
         * （`ToolPolicyUpdate` 里有），库里的值必须生效。
         */
        fun normalizeStored(config: ToolPolicyConfig) = config.copy(
            maxComfyQueriesPerRun = DEFAULT_MAX_COMFY_QUERIES_PER_RUN,
            maxWebQueriesPerRun = DEFAULT_MAX_WEB_QUERIES_PER_RUN,
            maxReadBytes = DEFAULT_MAX_READ_BYTES,
            maxWriteBytes = DEFAULT_MAX_WRITE_BYTES,
        )
    }
}

class ToolPolicy(
    val projectRoot: Path,
    val config: ToolPolicyConfig,
    /**
     * **自动发现的**只读目录（用户 bug ④："应该自动发现 comfy 目录，给 comfy 的目录默认白名单"）。
     *
     * 与 [ToolPolicyConfig.readRoots] 刻意分开两件事：
     *  - `readRoots` 是**用户配过的**（落库、界面可改、可删）；
     *  - 这一份是**每次现算的探测结果**（[com.comfyhub.ComfyRoots]），不落库 ——
     *    换了 ComfyUI 的安装位置、或者用户第一次装上 ComfyUI，下一次工具调用就自动认得。
     *
     * 它**只影响读**，写仍然只认 [writeRoots]。
     */
    val autoReadRoots: List<Path> = emptyList(),
) {
    val writeRoots: List<Path> = expand(config.writeRoots.ifEmpty { listOf(ToolPolicyConfig.DEFAULT_WRITE_DIR) })
    val readRoots: List<Path> = expand(config.readRoots.ifEmpty { ToolPolicyConfig.DEFAULT_READ_DIRS })

    /** 实际生效的读白名单：用户配的 + 自动发现的（去重，保持顺序）。 */
    val effectiveReadRoots: List<Path> = (readRoots + autoReadRoots).distinct()

    private fun expand(entries: List<String>): List<Path> = entries.mapNotNull { raw ->
        runCatching {
            val p = Paths.get(raw.trim())
            (if (p.isAbsolute) p else projectRoot.resolve(p)).normalize().toAbsolutePath()
        }.onFailure { LOG.warn("忽略非法路径配置 {}: {}", raw, it.message) }.getOrNull()
    }

    /**
     * 生效权限：用户覆盖 → 出厂档 → **再按权限档放宽**。
     *
     * 「自动允许（无需批准）」只把 `ask` 变成 `allow`：
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

    /** 自动允许（无需批准）：AI 不必再等批准。 */
    val fullPermission: Boolean get() = permissionMode == ToolPolicyConfig.PERMISSION_FULL

    /** 用户显式覆盖过档位（用于界面上的"已覆盖"标记）。 */
    fun isOverridden(tool: AgentTool): Boolean = ToolAccess.parse(config.overrides[tool.name]) != null

    /** 读路径：必须落在生效读白名单内（用户配的 + 自动发现的 ComfyUI 目录）。 */
    fun resolveRead(raw: String): Path = resolve(raw, effectiveReadRoots, "读")

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
            // 措辞要跟 verb 对上：这些段读写都不许碰，写成"不可改"会让一次读取被拒看起来像
            // "我能读、只是不能改"（实测被这条消息误导过一次）
            "拒绝$verb ${path.fileName}：$hit 是本项目的运行期数据 / 依赖目录，" +
                "任何工具都不许读也不许写（与用户白名单无关）"
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
            // 内置预算（查 ComfyUI 的次数）以代码为准，不认库里冻着的历史值 —— 见 normalizeStored
            // 用户机器上的 ComfyUI 目录每次现探（不落库）：读白名单要自动跟着它走（用户 bug ④）
            val auto = runCatching { com.comfyhub.ComfyRoots.autoReadRoots(projectRoot) }
                .onFailure { LOG.warn("探测 ComfyUI 目录失败：{}", it.message) }
                .getOrDefault(emptyList())
            return ToolPolicy(projectRoot, ToolPolicyConfig.normalizeStored(config), auto)
        }

        fun save(config: ToolPolicyConfig) {
            SettingsRepo.put(ToolPolicyConfig.KEY, AppJson.encodeToString(ToolPolicyConfig.serializer(), config))
        }

        /**
         * 一次性把**库里冻着的旧出厂值**抬到新出厂值（2026-09-22）。
         *
         * 为什么需要它：[save] 会把整份配置写进 `app_settings`，所以用户在 09-22 之前
         * 哪怕只是切过一次权限档，库里就冻着 `maxToolSteps: 8 / maxCallsPerRun: 16`。
         * 这两项**是用户可改的**，所以 [normalizeStored] 不许覆盖（那会把用户自己调的
         * 数字也一起改掉）—— 于是只能在这里**按值识别**：只有恰好等于旧出厂值时
         * （说明他从没改过、就是当年那份默认值）才抬上去。
         *
         * 幂等：抬过之后值不再是旧出厂值，再启动不会重复处理。返回是否真的改了。
         */
        fun bumpLegacyBudgets(): Boolean {
            val raw = runCatching { SettingsRepo.get(ToolPolicyConfig.KEY) }.getOrNull() ?: return false
            val config = runCatching { AppJson.decodeFromString(ToolPolicyConfig.serializer(), raw) }
                .getOrNull() ?: return false
            val bumped = bumpedFromLegacy(config) ?: return false
            LOG.info(
                "工具预算按新出厂值抬起：轮数 {} → {}，调用数 {} → {}",
                config.maxToolSteps, bumped.maxToolSteps, config.maxCallsPerRun, bumped.maxCallsPerRun,
            )
            save(bumped)
            return true
        }

        /**
         * [bumpLegacyBudgets] 的**纯函数**部分：只有恰好等于旧出厂值的那一项才抬，
         * 返回 null = 两项都不用动。抽出来是为了能不起数据库地钉住判据。
         */
        fun bumpedFromLegacy(config: ToolPolicyConfig): ToolPolicyConfig? {
            val steps = if (config.maxToolSteps == ToolPolicyConfig.LEGACY_MAX_TOOL_STEPS) {
                ToolPolicyConfig.DEFAULT_MAX_TOOL_STEPS
            } else {
                config.maxToolSteps
            }
            val calls = if (config.maxCallsPerRun == ToolPolicyConfig.LEGACY_MAX_CALLS_PER_RUN) {
                ToolPolicyConfig.DEFAULT_MAX_CALLS_PER_RUN
            } else {
                config.maxCallsPerRun
            }
            if (steps == config.maxToolSteps && calls == config.maxCallsPerRun) return null
            return config.copy(maxToolSteps = steps, maxCallsPerRun = calls)
        }
    }
}
