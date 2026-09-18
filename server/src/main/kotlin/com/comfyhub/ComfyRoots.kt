package com.comfyhub

import org.slf4j.LoggerFactory
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths

/**
 * ComfyUI 相关目录的自动发现（用户 bug ④）。
 *
 * 起因是一条真实对话：让 AI 去读用户指认的工作流文件
 *
 * ```
 * D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\user\default\workflows\krea2….json
 * ```
 *
 * 结果被工具权限挡了 —— `read_file` 的读白名单出厂只有 `<项目根>\comfyui` 与 `<项目根>\storage`，
 * 而**用户的 ComfyUI 根本不在项目里**（发布包解压到哪、ComfyUI 装在哪，两者毫无关系）。
 * 用户的原话是"应该自动发现 comfy 目录，给 comfy 的目录默认白名单"。
 *
 * 所以这里把"用户机器上的 ComfyUI 在哪"算出来，交给 [com.comfyhub.ai.tools.ToolPolicy] 当**只读**白名单。
 * 三条纪律：
 *
 *  1. **只放宽读，不放宽写**：写仍然只允许 `<根>\comfyui`（用户要求"默认不能修改 comfy 目录以外的内容"）。
 *     ComfyUI 的 input 目录由 `comfy_use_attachment` 那条专用通道写，不走 `write_file`。
 *  2. **认特征文件，不认名字**：判据与 [ComfyLocator.looksLikeComfy] 完全同一份
 *     （`main.py`+`comfy/`，或 `output/`+`models`/`input`）——认错目录的代价是"白名单里多一个没用的路径"，
 *     但更糟的是把它当成"ComfyUI 找到了"，所以宁可严。
 *  3. **有界扫描**：只在"已知 ComfyUI 目录的兄弟/叔伯目录"里翻，深度 ≤ [MAX_DEPTH]、
 *     目录访问数 ≤ [MAX_VISITED]，且只往名字里带 `comfy` 的分支下钻 —— 绝不全盘扫。
 *     为什么需要它：ComfyUI Desktop 把**程序装在一处、共享数据放在另一处**
 *     （本机实例：程序 `…\ComfyUI-Installs\ComfyUI\ComfyUI`，产物/模型 `…\ComfyUI-Shared`），
 *     只按配置的输出目录推是推不到工作流目录的。
 *
 * 探测结果是**建议**：用户仍然可以在「设置 → AI 工具权限」里手工加读目录；
 * 这里返回的路径会随设置变化**每次重新计算**（不落库），所以换了 ComfyUI 位置不用改配置。
 */
object ComfyRoots {
    private val log = LoggerFactory.getLogger(ComfyRoots::class.java)

    /** 兄弟目录里最多往下翻几层 */
    private const val MAX_DEPTH = 3

    /** 一次探测最多看多少个目录（防止在盘根上跑很久） */
    private const val MAX_VISITED = 3000

    private val ENV_KEYS = listOf("COMFYHUB_COMFY_HOME", "COMFYUI_HOME", "COMFYUI_PATH")

    /**
     * 除了出厂默认（`<根>\comfyui` + `<根>\storage`）之外，**还要放行只读**的本机 ComfyUI 目录。
     *
     * 返回的都是真实路径、去重、按"最可能命中"排序。找不到就是空列表（不是错误：
     * 用户可能根本没装 ComfyUI，那时读白名单就是出厂默认那两个）。
     */
    fun autoReadRoots(projectRoot: Path): List<Path> {
        val out = LinkedHashSet<Path>()
        return try {
            val seeds = seedRoots(projectRoot)
            seeds.forEach { out.add(it) }
            // 每一个已知的 ComfyUI 目录，都在它的"上一级"里找找同机的**其它**安装
            // （Desktop 的 Installs / Shared 分家就是靠这一步找到工作流目录的）
            seeds.forEach { seed ->
                val parent = seed.parent ?: return@forEach
                findNearby(parent).forEach { out.add(it) }
            }
            out.toList()
        } catch (e: Exception) {
            // 探测失败绝不能让"读白名单"变成空的 —— 那会让本来能读的 storage 也读不了
            log.warn("探测 ComfyUI 目录失败（忽略，只用出厂读白名单）：{}", e.message)
            out.toList()
        }
    }

    /**
     * 在 [start] 底下找 ComfyUI 目录（[autoReadRoots] 用的就是它）。
     *
     * 单独抽出来是为了能单测：ComfyUI Desktop 那套"程序装在一处、共享数据放在另一处"的布局
     * 是本机实测过的真实形状（`…\ComfyUI-Installs\ComfyUI\ComfyUI` 与 `…\ComfyUI-Shared`），
     * 而工作流文件恰恰躺在**前者**的 `user\default\workflows` 下 —— 只按配置的输出目录推是推不到的。
     */
    fun findNearby(start: Path): List<Path> {
        val out = LinkedHashSet<Path>()
        if (!Files.isDirectory(start)) return emptyList()
        scanNearby(start, out, depth = 1, budget = intArrayOf(MAX_VISITED))
        return out.toList()
    }

    /** 已知的 ComfyUI 根目录：环境变量 → 用户配置的输出目录 → 项目内的 `comfyui`。 */
    private fun seedRoots(projectRoot: Path): List<Path> {
        val out = LinkedHashSet<Path>()

        ENV_KEYS.forEach { key ->
            realDir(System.getenv(key))?.let { out.add(it) }
        }

        // 用户配的产物目录：它通常就是 `<ComfyUI>\output`，父目录就是 ComfyUI 根。
        // 单独包一层 runCatching：读库失败（首次启动还没建库 / 数据库没起来）不该让
        // 后面两条更可靠的来源（环境变量、项目内 comfyui）一起丢掉。
        runCatching { SettingsRepo.captureOutputDirOrNull() }.getOrNull()?.let { configured ->
            val dir = realDir(configured) ?: return@let
            val parent = dir.parent
            if (parent != null && looksLikeOutputDir(dir)) out.add(parent) else out.add(dir)
        }

        realDir(projectRoot.resolve("comfyui").toString())?.let { out.add(it) }
        return out.filter { Files.isDirectory(it) }
    }

    /** 目录名是不是 `output`（ComfyUI 的产物目录名是固定的）。 */
    private fun looksLikeOutputDir(dir: Path): Boolean =
        dir.fileName?.toString()?.equals("output", ignoreCase = true) == true

    private fun realDir(raw: String?): Path? {
        if (raw.isNullOrBlank()) return null
        return runCatching {
            val p = Paths.get(raw.trim()).toAbsolutePath().normalize()
            if (!Files.isDirectory(p)) null else p.toRealPath()
        }.getOrNull()
    }

    /**
     * 在 `start` 底下找 ComfyUI 目录。
     *
     * [depth] 从 1 开始；**第一层随便进，更深一层要求目录名里带 `comfy`** ——
     * 这一条同时解决两个问题：能找到 `ComfyUI-Installs\ComfyUI\ComfyUI` 这种三层嵌套，
     * 又不会翻进 `models\` 这种几百 GB 的目录。
     * 一旦某个目录被认成 ComfyUI 就不再往里翻（里面的 `custom_nodes\xxx` 不是 ComfyUI 根）。
     */
    private fun scanNearby(start: Path, out: MutableSet<Path>, depth: Int, budget: IntArray) {
        if (depth > MAX_DEPTH || budget[0] <= 0) return
        val children = runCatching {
            Files.newDirectoryStream(start).use { it.toList() }
        }.getOrNull() ?: return

        for (child in children.sortedBy { it.fileName.toString().lowercase() }) {
            if (budget[0]-- <= 0) return
            if (Files.isSymbolicLink(child)) continue
            if (!runCatching { Files.isDirectory(child) }.getOrDefault(false)) continue
            val real = runCatching { child.toRealPath() }.getOrNull() ?: continue

            val (ok, reason) = ComfyLocator.looksLikeComfy(real)
            if (ok) {
                log.info("自动放行只读的 ComfyUI 目录：{}（{}）", real, reason)
                out.add(real)
                continue
            }
            val nameLooksComfy = real.fileName?.toString()?.contains("comfy", ignoreCase = true) == true
            if (depth == 1 || nameLooksComfy) scanNearby(real, out, depth + 1, budget)
        }
    }
}
