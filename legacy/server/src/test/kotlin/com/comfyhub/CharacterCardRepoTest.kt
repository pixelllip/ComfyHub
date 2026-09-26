package com.comfyhub

import org.junit.jupiter.api.AfterAll
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * 角色卡（O8）的回归防线。
 *
 * 盯的是四件**删了以后出错也没人知道**的事：
 * ① 同名 = 同一张卡（幂等），且二次保存是**合并**不是覆盖 —— 覆盖会悄悄丢掉上一轮存的 tag 栈；
 * ② tag 拆分的分隔符（模型三种写法都会用）+ 去重保序；
 * ③ 超长 / 参考图不存在时**报错**而不是静默截断或写进去；
 * ④ 删产物之后卡还在、参考图自动置空（`ON DELETE SET NULL`，红线 R1 在这张新表上的落点）。
 *
 * 连真库（[TestDb] 是临时文件库，理由见 `SqliteFoundationTest.kt`）。
 */
class CharacterCardRepoTest {

    private fun save(input: CharacterCardInput) = Db.tx { conn -> CharacterCardRepo.save(conn, input) }

    @Test
    fun `存下来的卡能按名字 别名 tag LoRA 名搜到`() {
        TestDb.reset()
        val (id, created) = save(
            CharacterCardInput(
                name = "shinku",
                nameEn = "Shinku",
                aliases = listOf("shinku_(nte)", "真红"),
                characterTag = "shinku_(nte)",
                seriesTag = "neverness_to_everness",
                tags = CharacterCardRepo.splitTags(
                    "1girl, shinku_(nte), red_eyes, dragon_girl,\nblack hair, red streaks"
                ),
                loraName = "NTE(Anima)_trigger_word_@nte.safetensors",
                loraWeight = 0.7,
                source = "danbooru + 用户工作流 anima_shinku_nte_school",
            )
        )
        assertTrue(created)
        assertTrue(id > 0)

        // 五种关键字都要命中（判据是"模型会拿哪一种来搜"）
        for (q in listOf("shinku", "真红", "dragon_girl", "neverness", "NTE(Anima)")) {
            assertEquals(listOf(id), CharacterCardRepo.list(q = q).map { it.id }, "关键字 「$q」 没搜到")
        }
        // 搜不到的词回空列表（不是报错）
        assertTrue(CharacterCardRepo.list(q = "完全不存在的角色").isEmpty())

        val card = assertNotNull(CharacterCardRepo.get(id))
        assertEquals(6, card.tags.size)
        assertEquals("shinku_(nte)", card.tags[1])
        assertEquals(0.7, card.loraWeight)
    }

    @Test
    fun `同名再存是合并 不是覆盖 也不会攒出第二行`() {
        TestDb.reset()
        val (first, createdFirst) = save(
            CharacterCardInput(name = "iroi", tags = listOf("green_hair", "horns"), source = "civitai")
        )
        assertTrue(createdFirst)

        // 第二轮只补一个 LoRA 名（模型一次只说一件事）
        val (second, createdSecond) = save(CharacterCardInput(name = "iroi", loraName = "iroi-anima.safetensors"))
        assertEquals(first, second, "同名必须是同一张卡")
        assertTrue(!createdSecond, "第二次是更新，不是新建")

        val card = assertNotNull(CharacterCardRepo.get(first))
        assertEquals("iroi-anima.safetensors", card.loraName)
        assertEquals(listOf("green_hair", "horns"), card.tags, "上一轮存的 tag 栈不许被抹掉")
        assertEquals("civitai", card.source, "来源也不许被抹掉")
        assertEquals(1, CharacterCardRepo.list().size, "库里只该有一张 iroi")
    }

    @Test
    fun `tag 拆分认三种分隔符 去重保序 并洗掉列表符号`() {
        val tags = CharacterCardRepo.splitTags(
            "- 1girl， red_eyes\n2. dragon_girl\r\n* horns, 1girl\n\n , ,"
        )
        assertEquals(listOf("1girl", "red_eyes", "dragon_girl", "horns"), tags)
    }

    @Test
    fun `编辑是全量替换 清空就是清空`() {
        TestDb.reset()
        val (id, _) = save(
            CharacterCardInput(name = "k", tags = listOf("a", "b"), notes = "旧备注", source = "用户口述")
        )
        val ok = Db.tx { conn ->
            CharacterCardRepo.update(conn, id, CharacterCardInput(name = "k", tags = listOf("a")))
        }
        assertTrue(ok)
        val card = assertNotNull(CharacterCardRepo.get(id))
        assertEquals(listOf("a"), card.tags)
        assertNull(card.notes, "界面里的全量替换要能真的清掉字段")
        assertNull(card.source)
    }

    @Test
    fun `超长字段报错 不静默截断`() {
        TestDb.reset()
        val tooLong = "x".repeat(CharacterCardRepo.MAX_NOTES_CHARS + 1)
        val e = assertFailsWith<IllegalArgumentException> { save(CharacterCardInput(name = "k", notes = tooLong)) }
        assertTrue(e.message!!.contains("超出上限"), e.message!!)
        assertTrue(CharacterCardRepo.list().isEmpty(), "报错的行不许半截写进去")

        assertFailsWith<IllegalArgumentException> { save(CharacterCardInput(name = "   ")) }
    }

    @Test
    fun `参考图必须真的存在 删掉产物之后卡还在 参考图自动置空`() {
        TestDb.reset()
        val mediaId = Db.tx { conn ->
            conn.executeReturningKey(
                "INSERT INTO media_assets (kind, original_name, stored_name) VALUES ('IMAGE','a.png','a.png')"
            )
        }
        val (id, _) = save(CharacterCardInput(name = "有参考图", referenceMediaId = mediaId))
        assertEquals(mediaId, assertNotNull(CharacterCardRepo.get(id)).referenceMediaId)

        // 不存在的产物 → 人话报错（不写进去、也不留一个 500 的 SQL 原文）
        val e = assertFailsWith<IllegalArgumentException> {
            save(CharacterCardInput(name = "坏参考图", referenceMediaId = 999_999))
        }
        assertTrue(e.message!!.contains("不存在"), e.message!!)

        // R1：`ON DELETE SET NULL` 真的生效（外键必须在每条连接上开着）
        Db.tx { conn -> conn.execute("DELETE FROM media_assets WHERE id = ?", mediaId) }
        val card = assertNotNull(CharacterCardRepo.get(id), "删参考图不该把角色卡一起删掉")
        assertNull(card.referenceMediaId)
    }

    @Test
    fun `改一行会顶 updated_at 删了就查不到`() {
        TestDb.reset()
        val (id, _) = save(CharacterCardInput(name = "k"))
        Db.tx { conn ->
            conn.execute("UPDATE character_cards SET updated_at = ? WHERE id = ?", "2000-01-01T00:00:00.000Z", id)
        }
        Db.tx { conn -> CharacterCardRepo.update(conn, id, CharacterCardInput(name = "k", notes = "改了")) }
        assertTrue(
            assertNotNull(CharacterCardRepo.get(id)).updatedAt != "2000-01-01T00:00:00.000Z",
            "改了行必须顶 updated_at（触发器 trg_character_cards_touch）",
        )
        assertTrue(Db.tx { conn -> CharacterCardRepo.delete(conn, id) })
        assertNull(CharacterCardRepo.get(id))
    }

    @Test
    fun `角色卡不进同步范围 写一行不许往账本里落东西`() {
        TestDb.reset()
        val (id, _) = save(CharacterCardInput(name = "不该被同步"))
        Db.tx { conn -> CharacterCardRepo.update(conn, id, CharacterCardInput(name = "不该被同步", notes = "x")) }
        Db.tx { conn -> CharacterCardRepo.delete(conn, id) }
        val logged = Db.withConnection { conn ->
            conn.queryOne("SELECT COUNT(*) FROM _sync_log WHERE tbl = 'character_cards'") { it.getLong(1) }
        }
        assertEquals(0L, logged, "这张表刻意不同步（总纲 §3.3）；要同步必须连触发器与 sync.rs 一起改")
    }

    companion object {
        @AfterAll
        @JvmStatic
        fun cleanup() = TestDb.cleanup()
    }
}
