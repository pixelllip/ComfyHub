package com.comfyhub.ai.tools

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path

/**
 * Skills 仓库（M5 / AIH-037~045）。
 *
 * 两条需求驱动的设计，测试就盯着它们：
 *  1. **磁盘是正文真源**（没有缓存）—— save 之后马上 scan/read 就要看到；
 *  2. **非法项不静默忽略** —— 照样列进界面并带 `validationError`，但不进 `catalog()`、不能被 `read()`。
 *
 * 只用临时目录，不碰数据库也不联网。
 */
class SkillStoreTest {

    private class Env(val root: Path, val builtin: Path, val user: Path, val store: SkillStore)

    private fun env(): Env {
        val root = Files.createTempDirectory("comfyhub-skills").toRealPath()
        val builtin = root.resolve("builtin")
        val user = root.resolve("user")
        Files.createDirectories(builtin)
        Files.createDirectories(user)
        return Env(root, builtin, user, SkillStore(builtin, user))
    }

    /** 手写一份 SKILL.md（模拟"用户/AI 直接往磁盘里放"）。 */
    private fun writeSkill(file: Path, text: String) {
        Files.createDirectories(file.parent)
        Files.writeString(file, text, StandardCharsets.UTF_8)
    }

    // --- frontmatter --------------------------------------------------------

    @Test
    fun `parseFrontMatter 去引号 认注释 无围栏时整篇都是正文`() {
        val raw = """
            ---
            name: demo-skill
            description: "带引号的说明"
            whenToUse: '单引号也行'
            version: 1 # 行内注释
            # 整行注释
            ---
            正文第一行
            正文第二行
        """.trimIndent()
        val (meta, body) = SkillStore.parseFrontMatter(raw)
        assertEquals("demo-skill", meta["name"])
        assertEquals("带引号的说明", meta["description"])
        // 元数据 key 会被归一成小写（parseOne 就是按 meta["whentouse"] 取的）
        assertEquals("单引号也行", meta["whentouse"])
        assertEquals("1", meta["version"], "值里的行内注释要去掉")
        assertEquals("正文第一行\n正文第二行", body)

        // 没有开围栏 → 没有元数据，整篇都是正文（不能把第一段吃掉）
        val (none, all) = SkillStore.parseFrontMatter("没有 frontmatter 的文件\n第二行")
        assertTrue(none.isEmpty())
        assertEquals("没有 frontmatter 的文件\n第二行", all)

        // 有开围栏但没有闭合 → 同样当成没有元数据，绝不吞掉后半篇
        val (unclosed, raw2) = SkillStore.parseFrontMatter("---\nname: x\n正文")
        assertTrue(unclosed.isEmpty())
        assertEquals("---\nname: x\n正文", raw2)
    }

    @Test
    fun `parseFrontMatter 认 CRLF`() {
        val raw = "---\r\nname: crlf-skill\r\ndescription: 说明\r\n---\r\n正文 A\r\n正文 B\r\n"
        val (meta, body) = SkillStore.parseFrontMatter(raw)
        assertEquals("crlf-skill", meta["name"])
        assertEquals("说明", meta["description"])
        assertEquals("正文 A\n正文 B\n", body)
    }

    @Test
    fun `parseFrontMatter 认下划线写法与 BOM`() {
        val raw = "\uFEFF---\nname: x\ndescription: d\n---\nbody"
        val (meta, _) = SkillStore.parseFrontMatter(raw)
        assertEquals("x", meta["name"])
    }

    // --- save / scan / find / read ------------------------------------------

    @Test
    fun `save 落盘到用户根 之后 scan find read 立刻能往返`() {
        val e = env()
        val dto = e.store.save("demo-skill", "演示用", whenToUse = "做演示时", content = "步骤一\n步骤二")

        val file = e.user.resolve("demo-skill").resolve("SKILL.md")
        assertTrue(Files.isRegularFile(file), "必须写到 $file")
        assertTrue(Files.readString(file, StandardCharsets.UTF_8).contains("步骤一"))

        assertEquals("demo-skill", dto.name)
        assertEquals("user", dto.source)
        assertNull(dto.validationError)
        assertNotNull(dto.digest)

        val found = e.store.find("demo-skill")!!
        assertEquals("演示用", found.description)
        assertEquals("做演示时", found.whenToUse)

        val listed = e.store.scan().single()
        assertEquals("demo-skill", listed.name)
        assertTrue(e.store.catalog().any { it.name == "demo-skill" })

        val (read, body) = e.store.read("demo-skill")!!
        assertEquals("demo-skill", read.name)
        assertEquals("步骤一\n步骤二", body, "读出来的是去掉 frontmatter 的正文")
    }

    @Test
    fun `正文变化时 digest 跟着变`() {
        val e = env()
        val before = e.store.save("demo-skill", "演示用", content = "原来的正文")
        val after = e.store.save("demo-skill", "演示用", content = "改过的正文")
        assertTrue(before.digest != after.digest, "digest 必须能反映正文变更（$before vs $after）")
    }

    @Test
    fun `save 覆盖同名 skill 时不会残留旧正文`() {
        val e = env()
        e.store.save("demo-skill", "第一版", content = "第一版正文")
        e.store.save("demo-skill", "第二版", content = "第二版正文")
        val (dto, body) = e.store.read("demo-skill")!!
        assertEquals("第二版", dto.description)
        assertEquals("第二版正文", body)
        assertEquals(1, e.store.scan().size)
    }

    // --- 名称与体积校验 ------------------------------------------------------

    @Test
    fun `非法 skill 名一律拒绝 合法 kebab 名通过`() {
        val e = env()
        listOf("Bad_Name", "UPPER", "-leading", "", "有中文", "a".repeat(65)).forEach { bad ->
            val failure = assertFailsWith<ToolFailure>("「$bad」应当被拒") {
                e.store.save(bad, "说明", content = "正文")
            }
            assertEquals("INVALID_SKILL_NAME", failure.code, "「$bad」")
        }
        assertTrue(!Files.exists(e.user.resolve("a".repeat(65))), "被拒的不能留下半个目录")

        assertEquals("good-name-1", e.store.save("good-name-1", "说明", content = "正文").name)
        assertEquals("a".repeat(64), e.store.save("a".repeat(64), "说明", content = "正文").name, "64 字符是上限")
    }

    @Test
    fun `描述为空或过长 正文为空或过大 都被拒`() {
        val e = env()
        assertEquals(
            "INVALID_ARGUMENT",
            assertFailsWith<ToolFailure> { e.store.save("demo-skill", "   ", content = "正文") }.code,
        )
        assertEquals(
            "INVALID_ARGUMENT",
            assertFailsWith<ToolFailure> {
                e.store.save("demo-skill", "d".repeat(SkillStore.MAX_DESCRIPTION + 1), content = "正文")
            }.code,
        )
        assertEquals(
            "INVALID_ARGUMENT",
            assertFailsWith<ToolFailure> { e.store.save("demo-skill", "说明", content = "   ") }.code,
        )
        assertEquals(
            "INVALID_ARGUMENT",
            assertFailsWith<ToolFailure> {
                e.store.save("demo-skill", "说明", content = "x".repeat(SkillStore.MAX_BODY_BYTES + 1))
            }.code,
        )
    }

    // --- 非法项的呈现 -------------------------------------------------------

    @Test
    fun `缺 description 的 skill 会被列出但不可用`() {
        val e = env()
        writeSkill(e.user.resolve("no-desc").resolve("SKILL.md"), "---\nname: no-desc\nwhenToUse: 无\n---\n正文")

        val dto = e.store.find("no-desc")!!
        assertNotNull(dto.validationError, "非法项不能静默消失")
        assertTrue(dto.validationError.contains("description"), dto.validationError)
        assertTrue(e.store.scan().any { it.name == "no-desc" }, "列表里要能看到它（带诊断）")
        assertTrue(e.store.catalog().none { it.name == "no-desc" }, "非法项不进系统提示")

        val failure = assertFailsWith<ToolFailure> { e.store.read("no-desc") }
        assertEquals("SKILL_INVALID", failure.code)
    }

    @Test
    fun `frontmatter 的 name 与目录名不一致算非法`() {
        val e = env()
        writeSkill(e.user.resolve("alpha").resolve("SKILL.md"), "---\nname: beta\ndescription: 说明\n---\n正文")

        val dto = e.store.find("beta")!!
        assertNotNull(dto.validationError)
        assertTrue(dto.validationError.contains("不一致"), dto.validationError)
        assertTrue(e.store.catalog().none { it.name == "beta" })
    }

    @Test
    fun `正文为空也算非法`() {
        val e = env()
        writeSkill(e.user.resolve("empty-body").resolve("SKILL.md"), "---\nname: empty-body\ndescription: 说明\n---\n")
        val dto = e.store.find("empty-body")!!
        assertNotNull(dto.validationError)
        assertTrue(dto.validationError.contains("正文"), dto.validationError)
    }

    // --- 平铺写法与两个可调用开关 -------------------------------------------

    @Test
    fun `平铺 md 被识别 两个可调用开关都生效`() {
        val e = env()
        writeSkill(e.user.resolve("flat-one.md"), "---\nname: flat-one\ndescription: 平铺写法\n---\n正文")
        writeSkill(
            e.user.resolve("no-model").resolve("SKILL.md"),
            "---\nname: no-model\ndescription: 说明\ndisable-model-invocation: true\n---\n正文",
        )
        writeSkill(
            e.user.resolve("no-user").resolve("SKILL.md"),
            "---\nname: no-user\ndescription: 说明\nuser-invocable: false\n---\n正文",
        )

        val flat = e.store.find("flat-one")!!
        assertEquals("user", flat.source)
        assertEquals("平铺写法", flat.description)
        assertNull(flat.validationError)

        val noModel = e.store.find("no-model")!!
        assertNull(noModel.validationError)
        assertTrue(!noModel.modelInvocable, "disable-model-invocation: true ⇒ 模型不能自动用")
        assertTrue(noModel.userInvocable)

        val noUser = e.store.find("no-user")!!
        assertNull(noUser.validationError)
        assertTrue(!noUser.userInvocable, "user-invocable: false ⇒ 用户不能手动调")
        assertTrue(noUser.modelInvocable)
    }

    // --- 删除 ---------------------------------------------------------------

    @Test
    fun `delete 删掉用户 skill 的整个 bundle 但拒绝内置`() {
        val e = env()
        writeSkill(e.user.resolve("bundle-one").resolve("SKILL.md"), "---\nname: bundle-one\ndescription: 说明\n---\n正文")
        writeSkill(e.user.resolve("bundle-one").resolve("references/extra.md"), "参考")

        assertTrue(e.store.delete("bundle-one"))
        assertTrue(!Files.exists(e.user.resolve("bundle-one")), "bundle 目录（含 references）要一起删掉")
        assertNull(e.store.find("bundle-one"))
        assertTrue(e.store.scan().isEmpty())

        writeSkill(e.builtin.resolve("core-skill").resolve("SKILL.md"), "---\nname: core-skill\ndescription: 内置\n---\n正文")
        val failure = assertFailsWith<ToolFailure> { e.store.delete("core-skill") }
        assertEquals("SKILL_READONLY", failure.code)
        assertTrue(Files.isRegularFile(e.builtin.resolve("core-skill").resolve("SKILL.md")), "内置文件不能被删")
    }

    @Test
    fun `delete 平铺 md 只删文件 不误删用户根`() {
        val e = env()
        writeSkill(e.user.resolve("flat-two.md"), "---\nname: flat-two\ndescription: 说明\n---\n正文")
        assertTrue(e.store.delete("flat-two"))
        assertTrue(Files.isDirectory(e.user), "用户根必须还在")
        assertNull(e.store.find("flat-two"))
    }

    // --- 同名冲突 -----------------------------------------------------------

    @Test
    fun `同名时用户版本胜出并带冲突提示 目录里只剩一条`() {
        val e = env()
        writeSkill(e.builtin.resolve("dup-skill").resolve("SKILL.md"), "---\nname: dup-skill\ndescription: 内置版本\n---\n内置正文")
        writeSkill(e.user.resolve("dup-skill").resolve("SKILL.md"), "---\nname: dup-skill\ndescription: 用户版本\n---\n用户正文")

        val all = e.store.scan()
        assertEquals(1, all.count { it.name == "dup-skill" }, "同名只能出现一条")
        val winner = all.first { it.name == "dup-skill" }
        assertEquals("user", winner.source, "用户版本胜出")
        assertEquals("用户版本", winner.description)
        assertNotNull(winner.conflict, "覆盖内置要给出提示")

        assertEquals("用户正文", e.store.read("dup-skill")!!.second)
        assertEquals(1, e.store.catalog().count { it.name == "dup-skill" })
    }

    @Test
    fun `只有内置时正常列出且不可删`() {
        val e = env()
        writeSkill(e.builtin.resolve("core-skill").resolve("SKILL.md"), "---\nname: core-skill\ndescription: 内置\n---\n正文")
        val dto = e.store.scan().single()
        assertEquals("builtin", dto.source)
        assertNull(dto.conflict)
        assertEquals("正文", e.store.read("core-skill")!!.second)
    }

    @Test
    fun `找不到的 skill 一律返回 null 而不是抛错`() {
        val e = env()
        assertNull(e.store.read("nope"))
        assertNull(e.store.find("nope"))
        assertEquals(0, e.store.scan().size)
    }

    @Test
    fun `已有 skill 的描述很长仍然可用（长度不是错误）`() {
        // 真实例子：从 DSH 导入的 anima-nsfw-prompt 描述有 762 字。
        // 早先扫描时按 600 字判非法，等于把用户自己写好的 skill 直接禁用掉。
        val e = env()
        val long = "说明".repeat(400)
        writeSkill(
            e.user.resolve("long-desc").resolve("SKILL.md"),
            "---\nname: long-desc\ndescription: $long\n---\n正文",
        )

        val dto = e.store.find("long-desc")!!
        assertNull(dto.validationError, "描述长不该让 skill 变成非法项")
        assertEquals(1, e.store.catalog().count { it.name == "long-desc" }, "要能进系统提示目录")
        assertEquals("正文", e.store.read("long-desc")!!.second)
    }

    // --- 投放口：拷进来就算装好（用户建议：不要「从 DSH 导入」按钮）----------

    @Test
    fun `投放口：拷贝进来的平铺 md 没有 frontmatter 也会被自动登记`() {
        val e = env()
        writeSkill(e.user.resolve("my-helper.md"), "# 我的助手\n\n第一步：先这样\n第二步：再那样")

        val result = e.store.autoRegister()
        assertEquals(listOf("my-helper"), result.names)
        assertEquals(0, result.errors.size, result.errors.toString())

        // 补写的 frontmatter 要用文件名当 name、正文第一行当 description，正文一字不改
        val dto = e.store.find("my-helper")!!
        assertNull(dto.validationError)
        assertEquals("我的助手", dto.description)
        assertEquals(
            "# 我的助手\n\n第一步：先这样\n第二步：再那样",
            e.store.read("my-helper")!!.second,
            "补 frontmatter 不能动正文",
        )
        val raw = Files.readString(e.user.resolve("my-helper.md"), StandardCharsets.UTF_8)
        assertTrue(raw.startsWith("---\nname: my-helper\n"), raw.take(80))
    }

    @Test
    fun `投放口：bundle 目录（名字_SKILL_md）同样自动登记`() {
        val e = env()
        writeSkill(e.user.resolve("scene-prompt").resolve("SKILL.md"), "场景提示词规则")

        val result = e.store.autoRegister()
        assertEquals(listOf("scene-prompt"), result.names)
        assertEquals("场景提示词规则", e.store.find("scene-prompt")!!.description)
    }

    @Test
    fun `投放口：已经有 frontmatter 的文件一个字节都不动（哪怕它不合法）`() {
        val e = env()
        val original = "---\nname: broken-skill\n---\n正文" // 缺 description → 非法
        writeSkill(e.user.resolve("broken-skill").resolve("SKILL.md"), original)

        val result = e.store.autoRegister()
        assertEquals(0, result.registered)
        assertEquals(original, Files.readString(e.user.resolve("broken-skill").resolve("SKILL.md")))
        // 既有规矩不变：列出来、带诊断、不进 catalog
        assertNotNull(e.store.find("broken-skill")!!.validationError)
        assertTrue(e.store.catalog().none { it.name == "broken-skill" })
    }

    @Test
    fun `投放口：中文文件名没法转成 kebab-case 时如实报错 而不是静默忽略`() {
        val e = env()
        writeSkill(e.user.resolve("我的技能.md"), "正文")

        val result = e.store.autoRegister()
        assertEquals(0, result.registered)
        assertEquals(1, result.errors.size)
        assertTrue(result.errors.first().contains("kebab-case"), result.errors.first())
        // 文件保持原样，仍然在列表里（带 validationError）
        assertEquals("正文", Files.readString(e.user.resolve("我的技能.md"), StandardCharsets.UTF_8))
        assertNotNull(e.store.find("我的技能")!!.validationError)
    }

    @Test
    fun `投放口：空文件与非 md 文件不会被当成 skill`() {
        val e = env()
        writeSkill(e.user.resolve("empty-skill.md"), "   \n\n")
        writeSkill(e.user.resolve("readme.txt"), "这不是 skill")

        val result = e.store.autoRegister()
        assertEquals(0, result.registered)
        assertTrue(result.errors.any { it.contains("empty-skill") }, result.errors.toString())
        assertNull(e.store.find("readme"))
    }

    @Test
    fun `投放口：重复扫描是幂等的（第二次不会又登记一遍）`() {
        val e = env()
        writeSkill(e.user.resolve("idempotent.md"), "内容")
        assertEquals(1, e.store.autoRegister().registered)
        assertEquals(0, e.store.autoRegister().registered)
        assertEquals("内容", e.store.read("idempotent")!!.second)
    }

    @Test
    fun `投放口：ensureUserRoot 会把目录建出来（发布包里首次启动就靠它）`() {
        val root = Files.createTempDirectory("comfyhub-skills-fresh").toRealPath()
        val store = SkillStore(root.resolve("builtin"), root.resolve("storage").resolve("ai").resolve("skills"))
        val dir = store.ensureUserRoot()
        assertTrue(Files.isDirectory(dir))
        assertTrue(store.roots().userRootExists)
        assertTrue(store.roots().userRoot.endsWith("skills"))
    }

    // --- YAML 块标量（真实 SKILL.md 里到处都是 `description: |`）--------------

    @Test
    fun `description 写成块标量竖线时读出多行正文`() {
        val e = env()
        writeSkill(
            e.user.resolve("blocky").resolve("SKILL.md"),
            """
            ---
            name: blocky
            description: |
              第一行说明
              第二行说明
            version: 2
            ---
            正文
            """.trimIndent(),
        )
        val dto = e.store.find("blocky")!!
        assertNull(dto.validationError, dto.validationError)
        assertEquals("第一行说明\n第二行说明", dto.description)
        assertEquals("2", dto.version, "块标量结束后的同级 key 要继续读")
        assertEquals("正文", e.store.read("blocky")!!.second)
    }

    @Test
    fun `description 用折行符号时合成一行 去掉横杠时不留空行`() {
        val e = env()
        writeSkill(
            e.user.resolve("folded").resolve("SKILL.md"),
            "---\nname: folded\ndescription: >-\n  前半句\n  后半句\n---\n正文",
        )
        assertEquals("前半句 后半句", e.store.find("folded")!!.description)
    }

    @Test
    fun `真实形态：description 块标量 + 后面还有列表型 key`() {
        val e = env()
        writeSkill(
            e.user.resolve("realistic").resolve("SKILL.md"),
            """
            ---
            name: realistic
            description: |
              做视频用
              只在需要时加载
            allowed-tools:
            - webfetch
            - hub_image_search
            ---
            正文
            """.trimIndent(),
        )
        val dto = e.store.find("realistic")!!
        assertNull(dto.validationError, dto.validationError)
        assertEquals("做视频用\n只在需要时加载", dto.description)
        assertEquals("正文", e.store.read("realistic")!!.second)
        // 进系统提示时不能带换行（目录是一行一条）
        assertEquals("做视频用 只在需要时加载", dto.oneLineForPrompt)
    }
}
