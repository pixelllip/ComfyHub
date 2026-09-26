package com.comfyhub

import java.nio.file.Files
import java.nio.file.Path
import kotlin.io.path.createDirectories
import kotlin.io.path.createFile
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * ComfyUI 安装位置探测（用户"其他建议"第 3 条）。
 *
 * 这里只测**判据**：认错目录的代价是"捕获一条也收不到"，用户完全不知道为什么，
 * 所以宁可返回"不像"，也不要认一个乱七八糟的目录。
 */
class ComfyLocatorTest {

    private fun dir(vararg files: String): Path {
        val root = Files.createTempDirectory("comfy-locate")
        files.forEach { rel ->
            val p = root.resolve(rel)
            if (rel.endsWith("/")) p.createDirectories() else {
                p.parent?.createDirectories()
                p.createFile()
            }
        }
        return root
    }

    @Test
    fun `源码形态 main_py 加 comfy 包算`() {
        val d = dir("main.py", "comfy/")
        val (ok, reason) = ComfyLocator.looksLikeComfy(d)
        assertTrue(ok, reason)
    }

    @Test
    fun `任何形态 output 加 models 算`() {
        val d = dir("output/", "models/")
        assertTrue(ComfyLocator.looksLikeComfy(d).first)
    }

    @Test
    fun `output 加 input 也算（Desktop 整合包常见）`() {
        val d = dir("output/", "input/")
        assertTrue(ComfyLocator.looksLikeComfy(d).first)
    }

    @Test
    fun `只有空 output 目录不算（最容易误判的一种）`() {
        val d = dir("output/")
        val (ok, reason) = ComfyLocator.looksLikeComfy(d)
        assertFalse(ok)
        assertTrue(reason.contains("output"), reason)
    }

    @Test
    fun `有 main_py 但没有 comfy 包不算`() {
        val d = dir("main.py", "output/")
        assertFalse(ComfyLocator.looksLikeComfy(d).first)
    }

    @Test
    fun `空目录不算`() {
        assertFalse(ComfyLocator.looksLikeComfy(Files.createTempDirectory("empty")).first)
    }

    @Test
    fun `不存在的目录不算`() {
        assertFalse(ComfyLocator.looksLikeComfy(Path.of("Z:\\不存在的目录\\comfyui")).first)
    }

    @Test
    fun `系统目录不算（避免把 Windows 当工作目录的旧坑）`() {
        val windows = Path.of(System.getenv("SystemRoot") ?: "C:\\Windows")
        assertFalse(ComfyLocator.looksLikeComfy(windows).first)
    }
}
