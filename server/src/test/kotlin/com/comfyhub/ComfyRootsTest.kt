package com.comfyhub

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import java.nio.file.Files
import java.nio.file.Path

/**
 * 本机 ComfyUI 目录的自动发现（用户 bug ④）。
 *
 * 现场：用户甩给 AI 一个工作流路径
 * `D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\user\default\workflows\krea2….json`，
 * `read_file` 却因为读白名单只有"项目内的 comfyui + storage"而 PATH_DENIED。
 *
 * 这里钉住的是"能不能找到"这件事，以及**只找到该找的**：
 * 不把 `models\` 这种大目录当 ComfyUI 根、不把随便一个 `output\` 认成 ComfyUI
 * （认错的代价是让 AI 以为环境没问题）。
 */
class ComfyRootsTest {

    private fun tmp(): Path = Files.createTempDirectory("comfyhub-roots").toRealPath()

    /** ComfyUI Desktop 的真实布局：程序与共享数据分家。 */
    private fun desktopLayout(root: Path): Path {
        val install = root.resolve("ComfyUI-Installs/ComfyUI/ComfyUI")
        Files.createDirectories(install.resolve("comfy"))
        Files.createDirectories(install.resolve("user/default/workflows"))
        Files.writeString(install.resolve("main.py"), "# comfyui")
        Files.writeString(install.resolve("user/default/workflows/krea2.json"), "{}")

        val shared = root.resolve("ComfyUI-Shared")
        Files.createDirectories(shared.resolve("output"))
        Files.createDirectories(shared.resolve("models"))
        Files.createDirectories(shared.resolve("input"))
        return install
    }

    @Test
    fun `桌面版布局里能找到工作流所在的那个安装目录`() {
        val root = tmp()
        val install = desktopLayout(root)

        val found = ComfyRoots.findNearby(root)
        assertTrue(
            found.any { it.toRealPath() == install.toRealPath() },
            "没找到安装目录（工作流就在它下面的 user/default/workflows）：$found",
        )
    }

    @Test
    fun `共享目录也认但不会往里钻`() {
        val root = tmp()
        desktopLayout(root)

        val found = ComfyRoots.findNearby(root).map { it.fileName.toString() }
        assertTrue(found.contains("ComfyUI-Shared"), "有 output/ 与 models/ 的共享目录也是可读的 ComfyUI 目录：$found")
        // 认出来就不再往里翻：models 里几百 GB，翻进去纯属浪费
        assertTrue(found.none { it == "models" || it == "output" }, "不该把子目录也当成 ComfyUI 根：$found")
    }

    @Test
    fun `只有一个空 output 的目录不算 ComfyUI`() {
        val root = tmp()
        val fake = root.resolve("MyStuff")
        Files.createDirectories(fake.resolve("output"))
        Files.writeString(fake.resolve("output/keep.txt"), "x")

        assertTrue(ComfyRoots.findNearby(root).isEmpty(), "只有一个 output/ 不算 ComfyUI（判据见 ComfyLocator.looksLikeComfy）")
    }

    @Test
    fun `名字里不带 comfy 的深目录不再下钻`() {
        val root = tmp()
        val deep = root.resolve("Vendor/Resources/ComfyUI")
        Files.createDirectories(deep.resolve("comfy"))
        Files.writeString(deep.resolve("main.py"), "# comfyui")

        // 第 2 层叫 Resources（不含 comfy），第 3 层才叫 ComfyUI —— 按规则不再往下翻
        assertTrue(ComfyRoots.findNearby(root).isEmpty(), "只在名字带 comfy 的分支里下钻，避免全盘扫描")
    }

    @Test
    fun `找不到就是空列表 不是异常`() {
        val root = tmp()
        Files.createDirectories(root.resolve("nothing-here"))
        assertEquals(emptyList(), ComfyRoots.findNearby(root))
        assertEquals(emptyList(), ComfyRoots.findNearby(root.resolve("does-not-exist")))
    }
}
