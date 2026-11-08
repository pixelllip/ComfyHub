package com.comfyhub.harness.tools

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.File
import java.io.InputStream
import java.nio.file.Files
import java.nio.file.Path

/**
 * `SkillSeeder`：把随包内置的提示词 skill 种进手机投放口。
 *
 * 用例盯的是**四条纪律**（都在 `SkillSeeder` 的文件头里）：
 *  只种一次 / 绝不覆盖 / 失败不写标记 / 用户删掉的不复活。
 * 这四条错了都**不报错**，只是"手机上少一份规则"或者"用户改的被悄悄盖回去"。
 */
class SkillSeederTest {

    private lateinit var root: Path
    private lateinit var assets: File
    private lateinit var target: File

    /** 把 `assets/` 当普通目录树用的资产源（真机上是 APK 的 AssetManager）。 */
    private class DirAssetSource(private val root: File) : SkillAssetSource {
        override fun list(assetPath: String): List<String> =
            File(root, assetPath).listFiles()?.map { it.name }?.sorted().orEmpty()

        override fun open(assetPath: String): InputStream? {
            val file = File(root, assetPath)
            return if (file.isFile) file.inputStream() else null
        }
    }

    private fun source() = DirAssetSource(assets)

    private fun write(relative: String, text: String) {
        val file = File(root.toFile(), relative)
        file.parentFile?.mkdirs()
        file.writeText(text, Charsets.UTF_8)
    }

    @Before
    fun setUp() {
        root = Files.createTempDirectory("skill-seeder")
        assets = File(root.toFile(), "assets")
        target = File(root.toFile(), "target/ai/skills")
        // 两份目录版（其中一份带 references）+ 一份平铺：仓库 `skills/builtin/` 的两种布局都认
        write("assets/builtin/r2i-prompt/SKILL.md", "---\nname: r2i-prompt\ndescription: 扩写\n---\n正文")
        write("assets/builtin/krea-2/SKILL.md", "---\nname: krea-2\ndescription: 写提示词\n---\n正文")
        write("assets/builtin/krea-2/references/official-prompting.md", "官方原文")
        write("assets/builtin/flat-one.md", "---\nname: flat-one\ndescription: 平铺\n---\n正文")
    }

    @After
    fun tearDown() {
        runCatching {
            Files.walk(root).sorted(Comparator.reverseOrder()).forEach { Files.deleteIfExists(it) }
        }
    }

    @Test
    fun `第一次种：目录版、平铺与 references 都到位`() {
        val result = SkillSeeder.seed(source(), target, listOf("r2i-prompt", "krea-2", "flat-one"))

        assertEquals("4 个文件：r2i 1 + krea 2 + 平铺 1", 4, result.created)
        assertEquals(0, result.skipped)
        assertEquals(0, result.failed)
        assertTrue(File(target, "r2i-prompt/SKILL.md").isFile)
        assertTrue("references 要跟着过去（手机以后要读它就有）", File(target, "krea-2/references/official-prompting.md").isFile)
        assertTrue("平铺版落在投放口根部", File(target, "flat-one.md").isFile)
        assertTrue("种完要留标记", File(target, ".seeded").isFile)
    }

    @Test
    fun `第二次不再动：标记在就一个字节都不写`() {
        SkillSeeder.seed(source(), target, listOf("r2i-prompt"))
        val seeded = File(target, "r2i-prompt/SKILL.md")
        seeded.writeText("用户改过的正文", Charsets.UTF_8)

        val again = SkillSeeder.seed(source(), target, listOf("r2i-prompt"))
        assertFalse("标记在就整趟跳过", again.ran)
        assertEquals("用户改的那份不许被盖回去", "用户改过的正文", seeded.readText(Charsets.UTF_8))
    }

    @Test
    fun `标记被清掉后重种：已存在的文件只跳过、不覆盖`() {
        SkillSeeder.seed(source(), target, listOf("r2i-prompt", "krea-2"))
        val edited = File(target, "krea-2/SKILL.md")
        edited.writeText("用户改过的正文", Charsets.UTF_8)
        File(target, ".seeded").delete()

        val result = SkillSeeder.seed(source(), target, listOf("r2i-prompt", "krea-2"))
        assertEquals("一条都不该新建", 0, result.created)
        assertEquals("三份都已存在", 3, result.skipped)
        assertEquals("用户改过的正文原样留着", "用户改过的正文", edited.readText(Charsets.UTF_8))
    }

    @Test
    fun `资源里没有的 skill 记失败且不写标记（下次还会再试）`() {
        val result = SkillSeeder.seed(source(), target, listOf("r2i-prompt", "renamed-away"))
        assertEquals(1, result.created)
        assertEquals("改名的那个要如实计数，不许当成种好了", 1, result.failed)
        assertFalse("有失败就不写标记 —— 否则手机上永远是半个投放口", File(target, ".seeded").exists())

        // 下次把名字改回来：能正常补上
        val retry = SkillSeeder.seed(source(), target, listOf("r2i-prompt"))
        assertEquals(0, retry.created)
        assertEquals(1, retry.skipped)
        assertEquals(0, retry.failed)
        assertTrue(File(target, ".seeded").isFile)
    }

    @Test
    fun `用户删掉的 skill 不会自己长回来`() {
        SkillSeeder.seed(source(), target, listOf("r2i-prompt"))
        File(target, "r2i-prompt/SKILL.md").delete()

        SkillSeeder.seed(source(), target, listOf("r2i-prompt"))
        assertFalse(
            "用户删掉是用户的选择；自动复活会让人以为删不掉",
            File(target, "r2i-prompt/SKILL.md").exists(),
        )
    }
}
