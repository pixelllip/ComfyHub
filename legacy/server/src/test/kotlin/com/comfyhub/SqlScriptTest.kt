package com.comfyhub

import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * DDL 切分的防线。
 *
 * 为什么值得单独一个用例：`db/schema.sqlite.sql` 里的触发器**体内就有分号**，
 * 按 `split(";")` 硬切会把一个触发器切成两半，而报错是 `near "END": syntax error` ——
 * 看的人百分百不会往"切句"上想。这里直接拿**真文件**过一遍。
 */
class SqlScriptTest {

    private fun schemaSql(): String {
        val candidates = listOf(Path.of("../db/schema.sqlite.sql"), Path.of("db/schema.sqlite.sql"))
        val file = candidates.firstOrNull { Files.isRegularFile(it) }
            ?: error("找不到 db/schema.sqlite.sql（测试工作目录=${Path.of("").toAbsolutePath()}）")
        return Files.readString(file)
    }

    @Test
    fun `触发器整段一句：体内的分号不切句`() {
        val statements = SqlScript.split(schemaSql())
        val triggers = statements.filter { it.contains("CREATE TRIGGER", ignoreCase = true) }
        assertTrue(triggers.size >= 30, "触发器数量看起来不对：${triggers.size}")
        triggers.forEach { stmt ->
            assertTrue(
                stmt.trimEnd().endsWith("END", ignoreCase = true),
                "触发器被切碎了（结尾不是 END）：…${stmt.takeLast(60)}",
            )
        }
        assertTrue(statements.any { it.contains("CREATE TABLE IF NOT EXISTS prompts") })
        assertTrue(statements.any { it.contains("CREATE TABLE IF NOT EXISTS _sync_log") })
        assertEquals(1, statements.count { it.contains("PRAGMA user_version") })
    }

    @Test
    fun `单引号里的分号与注释里的分号都不算语句结束`() {
        val sql = """
            -- 注释里有一个分号 ; 不该切句
            CREATE TABLE a (x TEXT DEFAULT 'a;b');
            /* 块注释里也有一个 ; */
            CREATE TABLE b (y TEXT);
        """.trimIndent()
        val statements = SqlScript.split(sql)
        assertEquals(2, statements.size, "实际切出：$statements")
        assertTrue(statements[0].contains("'a;b'"), "单引号里的分号必须原样保留")
        assertTrue(statements[1].contains("CREATE TABLE b"))
    }
}
