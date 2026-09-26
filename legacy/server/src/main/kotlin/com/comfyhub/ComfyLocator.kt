package com.comfyhub

import kotlinx.serialization.Serializable
import org.slf4j.LoggerFactory
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths

/**
 * ComfyUI 安装位置探测（用户"其他建议"第 3 条：「考虑发行版如何自动获取 comfy 所在目录」）。
 *
 * 为什么需要它：源码树上我们知道 ComfyUI 在哪（`<根>\comfyui`），但**发布包是便携式的** ——
 * 用户可能把包解压到任何地方，ComfyUI 则装在自己习惯的位置（ComfyUI Desktop 的默认目录、
 * 免安装 zip、秋叶整合包…）。让他手填一个绝对路径既不友好也容易填错。
 *
 * 探测是**只读**的，且**只认特征文件**（`main.py` + `comfy/` 或 `output/`），
 * 不会"看到一个叫 comfyui 的文件夹就认"：
 * 认错目录的代价是捕获一条也收不到，而用户完全不知道为什么。
 *
 * 探测结果**不会自动生效**：`find()` 只负责回答"在哪"，
 * 是否用它由用户点「使用这个目录」决定（设置页 → ComfyUI 自动捕获）。
 */
@Serializable
data class ComfyLocation(
    /** 探测到的 ComfyUI 根目录（找不到时为 null）。 */
    val home: String? = null,
    /** 大概率能直接读产物的输出目录（`<home>\output`）。 */
    val outputDir: String? = null,
    /** 从哪里找到的（给用户看的一句话）。 */
    val source: String? = null,
    /** 当前设置里**已经**生效的输出目录（没设置时为 null）。 */
    val configuredOutputDir: String? = null,
    /** 探测到的候选（含被否决的原因），排错用。 */
    val candidates: List<ComfyCandidate> = emptyList(),
    val note: String? = null,
)

@Serializable
data class ComfyCandidate(
    val path: String,
    val kind: String,
    /** 这个目录看起来是不是一个 ComfyUI 安装（有特征文件）。 */
    val looksLikeComfy: Boolean = false,
    /** 判定依据 / 否决原因（只读展示）。 */
    val reason: String? = null,
)

/**
 * 按"最可能命中"的顺序找 ComfyUI（每个候选都要过 [looksLikeComfy] 这一关）。
 */
object ComfyLocator {
    private val log = LoggerFactory.getLogger(ComfyLocator::class.java)

    /** 特征：ComfyUI 一定有这些之一。 */
    private const val MAIN = "main.py"
    private const val COMFY_PKG = "comfy"
    private const val OUTPUT = "output"

    fun find(cfg: AppConfig): ComfyLocation {
        val candidates = mutableListOf<ComfyCandidate>()
        val configured = runCatching { SettingsRepo.captureConfig(cfg).outputDir }
            .getOrNull()
            ?.takeIf { it.isNotBlank() }

        fun consider(path: Path?, kind: String): Path? {
            if (path == null) return null
            val exists = Files.isDirectory(path)
            if (!exists) return null
            val (ok, reason) = looksLikeComfy(path)
            candidates += ComfyCandidate(path.toString(), kind, ok, reason)
            return if (ok) path else null
        }

        // 1) 用户已经配过：尊重它，不再探（配错了由用户自己改）
        if (configured != null) {
            val dir = Paths.get(configured)
            // 配的是 output 时，home 就是它的父目录
            val home = if (dir.fileName?.toString().equals(OUTPUT, ignoreCase = true)) dir.parent else dir
            return ComfyLocation(
                home = home?.toString(),
                outputDir = configured,
                source = "已经配置过了（就按这个用）",
                configuredOutputDir = configured,
                candidates = candidates,
            )
        }

        // 2) 显式环境变量（脚本 / 高级用户）
        val home = consider(env("COMFYHUB_COMFY_HOME"), "环境变量 COMFYHUB_COMFY_HOME")
            ?: consider(env("COMFYUI_HOME"), "环境变量 COMFYUI_HOME")
            ?: consider(env("COMFYUI_PATH"), "环境变量 COMFYUI_PATH")

        // 3) 项目根下的 comfyui（源码树 / 发布包都能带上这一份）
        val local = consider(cfg.projectRoot.resolve("comfyui"), "项目内的 comfyui 目录")

        // 4) 常见安装位置
        val located = home
            ?: local
            ?: consider(Paths.get("D:\\ComfyUI"), "常见位置 D:\\ComfyUI")
            ?: consider(Paths.get("D:\\ComfyUI_windows_portable\\ComfyUI"), "免安装包 D:\\ComfyUI_windows_portable")
            ?: searchUserProfile()
            ?: searchDesktopInstalls()
            ?: searchDesktopPackages()

        if (located == null) {
            return ComfyLocation(
                configuredOutputDir = null,
                candidates = candidates,
                note = "没找到 ComfyUI。可以手工把它的 output 目录填到「ComfyUI 自动捕获」里" +
                    "（ComfyUI 界面里点齿轮 → 看「输出目录」），或者设一个 COMFYHUB_COMFY_HOME 环境变量。",
            )
        }

        val output = located.resolve(OUTPUT).takeIf { Files.isDirectory(it) }?.toString()
            ?: located.resolve(OUTPUT).toString()
        val source = candidates.lastOrNull { it.looksLikeComfy }?.kind
        log.info("探测到 ComfyUI：{}（输出目录 {}）", located, output)
        return ComfyLocation(
            home = located.toString(),
            outputDir = output,
            source = source,
            configuredOutputDir = null,
            candidates = candidates,
        )
    }

    private fun env(name: String): Path? =
        System.getenv(name)?.takeIf { it.isNotBlank() }?.let { runCatching { Paths.get(it) }.getOrNull() }

    /** `%USERPROFILE%\Documents\ComfyUI`（ComfyUI Desktop 的默认安装位置）。 */
    private fun searchUserProfile(): Path? {
        val home = System.getProperty("user.home") ?: return null
        val base = Paths.get(home)
        val tries = listOf(
            base.resolve("Documents").resolve("ComfyUI"),
            base.resolve("ComfyUI"),
            // ComfyUI Desktop（新版）常把源码检出放在 Documents\comfy\ComfyUI
            base.resolve("Documents").resolve("comfy").resolve("ComfyUI"),
            base.resolve("AppData").resolve("Local").resolve("Programs").resolve("@comfyorgcomfyui-electron"),
        )
        return tries.firstOrNull { Files.isDirectory(it) && looksLikeComfy(it).first }
    }

    /**
     * ComfyUI **Desktop**（新版 Electron 启动器）的嵌套布局（2026-09-21 实测踩到）。
     *
     * 它把源码检出放在 `<应用目录>\ComfyUI-Installs\<实例名>\ComfyUI` 下，
     * 而不是 `Documents\ComfyUI` —— 只在顶层翻一层永远找不到它，
     * 而用户机器上的 ComfyUI 恰恰就是这么装的（实测：
     * `D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI`，有 `main.py` + `comfy/`）。
     *
     * 探测**有界**：固定几个应用目录 → 每个目录下的 `ComfyUI-Installs\*` 再拼一层 `ComfyUI`，
     * 不做全盘扫描（与 [ComfyRoots] 的"搜索有界"是同一条纪律）。
     * 判据不变：仍然要过 [looksLikeComfy]，找不到就如实返回 null。
     *
     * ⚠ 这份判据与 `scripts\comfy-path.ps1` 的 `Resolve-ComfyHome` **必须一致**
     * （AGENTS §10.1）：改一边就要改另一边，否则会出现"App 说找到了、脚本说找不到"。
     */
    private fun searchDesktopInstalls(): Path? {
        // 显式标类型（别让编译器从 chain 里推）：这两个可能拿不到，下面要用它们拼候选
        val home: Path? = System.getenv("USERPROFILE")?.let { p -> runCatching { Paths.get(p) }.getOrNull() }
        val localAppData: Path? = System.getenv("LOCALAPPDATA")?.let { p -> runCatching { Paths.get(p) }.getOrNull() }

        val roots = mutableListOf<Path>()
        if (localAppData != null) {
            roots.add(localAppData.resolve("Programs").resolve("@comfyorgcomfyui-electron"))
            roots.add(localAppData.resolve("Comfy-Desktop"))
        }
        if (home != null) roots.add(home.resolve("Documents").resolve("comfy"))
        runCatching { Paths.get("D:\\Comfy-Desktop") }.getOrNull()?.let { roots.add(it) }
        for (root in roots) {
            if (!Files.isDirectory(root)) continue
            // (a) <root>\ComfyUI-Installs\<实例>\ComfyUI   (b) <root>\<实例>\ComfyUI
            for (base in listOf(root.resolve("ComfyUI-Installs"), root)) {
                if (!Files.isDirectory(base)) continue
                val hit = runCatching {
                    Files.list(base).use { stream ->
                        // 两级判定分开写，别让编译器去推 Path? 的智能转换
                        stream.toList()
                            .mapNotNull { it?.resolve("ComfyUI") }
                            .firstOrNull { looksLikeComfy(it).first }
                    }
                }.getOrNull()
                if (hit != null) return hit
            }
        }
        return null
    }

    /** 秋叶整合包 / 便携包常见的 `*ComfyUI*` 顶层目录（只翻一层，避免全盘扫描）。 */
    private fun searchDesktopPackages(): Path? {
        val roots = listOfNotNull(
            System.getenv("USERPROFILE")?.let { Paths.get(it, "Desktop") },
            System.getenv("USERPROFILE")?.let { Paths.get(it, "Documents") },
            System.getenv("USERPROFILE")?.let { Paths.get(it, "Downloads") },
            runCatching { Paths.get("D:\\") }.getOrNull(),
        )
        for (root in roots) {
            if (!Files.isDirectory(root)) continue
            val found = runCatching {
                Files.list(root).use { stream ->
                    stream.filter { Files.isDirectory(it) }
                        .filter { it.fileName.toString().contains("comfy", ignoreCase = true) }
                        .filter { looksLikeComfy(it).first }
                        .findFirst()
                        .orElse(null)
                }
            }.getOrNull()
            if (found != null) return found
        }
        return null
    }

    /**
     * 这个目录看起来是不是 ComfyUI。
     *
     * 判据刻意严一点：`main.py` + `comfy/` 是**源码形态**，`output/` + `input/` + `models/`
     * 是**任何形态**（含 Desktop / 整合包）都有的。只有一个空 `output` 不算 ——
     * 用户随便建个 output 目录就会被误判，然后"捕获一条也收不到"。
     */
    fun looksLikeComfy(dir: Path): Pair<Boolean, String> {
        val hasMain = Files.isRegularFile(dir.resolve(MAIN))
        val hasPkg = Files.isDirectory(dir.resolve(COMFY_PKG))
        val hasOutput = Files.isDirectory(dir.resolve(OUTPUT))
        val hasModels = Files.isDirectory(dir.resolve("models"))
        val hasInput = Files.isDirectory(dir.resolve("input"))
        return when {
            hasMain && hasPkg -> true to "有 main.py 与 comfy/（源码形态）"
            hasOutput && (hasModels || hasInput) -> true to "有 output/ 与 models/ 或 input/"
            hasMain -> false to "有 main.py 但没有 comfy/（可能不是 ComfyUI 根目录）"
            hasOutput -> false to "只有 output/，没有 models/ 或 input/（不太像 ComfyUI 根目录）"
            else -> false to "没有 main.py / comfy/ / output/"
        }
    }
}
