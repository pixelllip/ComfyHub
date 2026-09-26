package com.comfyhub

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * 图生图投放的文件名规则（用户 bug：工作流的 `LoadImage` 读的是它自己被捕获时绑定的那张 jpg，
 * 模型改不了那个名字）。
 *
 * 名字必须同时满足两件事，否则会出很难查的错：
 *  1. **同一个附件永远得到同一个名字** —— 重跑工作流时那个文件还在（不然 ComfyUI 报 invalid image file）；
 *  2. **不同附件即使原文件名一样也不互相覆盖** —— 两个用户都叫 `image.png` 是常态。
 */
class AiWorkflowSearchTest {

    @Test
    fun `同一个附件得到同样的名字 不同附件即使同名也不会撞`() {
        val a = AiWorkflowSearch.inputFilenameFor("image.png", "a1b2c3d4e5f6", null)
        val again = AiWorkflowSearch.inputFilenameFor("image.png", "a1b2c3d4e5f6", null)
        assertEquals(a, again, "重复投放必须落在同一个文件上")

        val other = AiWorkflowSearch.inputFilenameFor("image.png", "ffffffff0000", null)
        assertTrue(a != other, "不同附件不能互相覆盖：$a vs $other")

        assertTrue(a.startsWith("image-"), a)
        assertTrue(a.endsWith(".png"), a)
        assertEquals(8, a.removePrefix("image-").removeSuffix(".png").length)
    }

    @Test
    fun `文件名里的路径分隔符与怪字符一律清掉`() {
        val name = AiWorkflowSearch.inputFilenameFor("../../etc/pass\\wd?.png", "abcd1234", null)
        // 只要没有路径分隔符，它就只是一个普通文件名（ComfyUI 的 input 目录里不会跑出去）
        assertTrue(!name.contains("/"), name)
        assertTrue(!name.contains("\\"), name)
        assertTrue(!name.contains("?"), name)
        assertTrue(!name.startsWith("."), name)
        assertTrue(!name.contains(':'), name)
    }

    @Test
    fun `没有扩展名与用户指定名字都能用`() {
        val noExt = AiWorkflowSearch.inputFilenameFor("photo", "abcd1234", null)
        assertTrue(noExt.startsWith("photo-abcd1234"), noExt)

        val wanted = AiWorkflowSearch.inputFilenameFor("photo.png", "abcd1234", "reference.jpg")
        assertTrue(wanted.startsWith("reference-"), wanted)
        assertTrue(wanted.endsWith(".jpg"), wanted)
    }

    @Test
    fun `空名字也有兜底 不会生成一个空文件名`() {
        val built = AiWorkflowSearch.inputFilenameFor("", "abcd1234", "")
        assertTrue(built.isNotBlank())
        assertTrue(built.startsWith("image"), built)
    }
}
