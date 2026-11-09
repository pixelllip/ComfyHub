package com.comfyhub.harness.store

import androidx.sqlite.driver.bundled.BundledSQLiteDriver

/**
 * 手机端本地库的**唯一入口**：开连接 → 上 PRAGMA → 建表 → 版本迁移 → 自检。
 *
 * 三条纪律（都不是可选项）：
 *  1. **开连接就走 PRAGMA**（R1：`foreign_keys` 按连接生效，漏了外键静默失效）；
 *  2. **自检失败就抛**，不许"带着坏掉的库继续跑" —— `foreign_keys=0` 的库表现是
 *     "删会话不级联"，等用户发现时数据已经乱了；
 *  3. 迁移风格照搬桌面 `Migrate.kt`：**幂等**（全部 `IF NOT EXISTS`）+ `PRAGMA user_version`
 *     记结构版本，不引 Flyway/Room 那套。
 */
class HarnessDatabase private constructor(val session: SqlSession) : AutoCloseable {

    override fun close() = session.close()

    companion object {
        /** 库文件名（放在 App 私有目录下，见 platform channel 的注入点）。 */
        const val DB_FILE = "comfyhub-harness.db"

        /**
         * 手机运行时用这个：**bundled 引擎**（R14），库文件路径由调用方给（App 私有目录）。
         */
        fun open(fileName: String = DB_FILE): HarnessDatabase =
            from(AndroidSqlSession.open(BundledSQLiteDriver(), fileName))

        /**
         * 从任意 [SqlSession] 起库（单测传 JDBC 实现 —— 跑的是同一份 DDL 与 PRAGMA）。
         */
        fun from(session: SqlSession): HarnessDatabase {
            val problems = MobilePragmas.problems(session)
            check(problems.isEmpty()) { "本地库自检不通过：\n - " + problems.joinToString("\n - ") }

            val current = userVersion(session)
            MobileSchema.DDL.forEach { session.exec(it) }
            // `CREATE TABLE IF NOT EXISTS` **不会**给已经存在的表补列 —— 旧版手机升级上来时
            // 必须在这里补。不补的后果不是报错，是"每次启动先把镜像清空、然后一行都落不回来"：
            // 桌面 SCHEMA_VERSION 13 的行带着这些列，`applyMirrorRow` 见到不认识的列会整批拒绝。
            migrateColumns(session)
            if (current < MobileSchema.VERSION) {
                session.exec("PRAGMA user_version = ${MobileSchema.VERSION}")
            }
            return HarnessDatabase(session)
        }

        /**
         * 旧库补列（幂等）：桌面 schema 加过、手机镜像还没跟上的那几列。
         *
         * 只加不删、不重建：镜像行由 bootstrap / changes 整行覆盖，补上列之后下一次同步就齐了。
         * 表本身不存在时跳过（DDL 已经建好，第一次启动不需要 ALTER）。
         */
        private fun migrateColumns(session: SqlSession) {
            NEW_COLUMNS.forEach { (table, columns) ->
                val existing = session.query("PRAGMA table_info($table)")
                    .map { it.str("name") }
                    .toSet()
                if (existing.isEmpty()) return@forEach
                columns.forEach { (name, decl) ->
                    if (name !in existing) {
                        session.exec("ALTER TABLE $table ADD COLUMN $name $decl")
                    }
                }
            }
        }

        /**
         * `table -> (列名, 列声明)`。加列时**两边一起改**：这里的 DDL 与 [MobileSchema] 里的建表语句。
         *
         * 2026-11-08 补的是桌面 5..13 之间积下来的四列 + `media_collections` 那张表
         * （表由 DDL 建，这里只补列）。
         */
        private val NEW_COLUMNS: Map<String, List<Pair<String, String>>> = mapOf(
            "prompts" to listOf("negative_mode" to "TEXT NOT NULL DEFAULT 'keep'"),
            "media_assets" to listOf(
                "collection_id" to "INTEGER NULL REFERENCES media_collections(id) ON DELETE SET NULL",
                "collection_opt_out" to "INTEGER NOT NULL DEFAULT 0",
            ),
            "ai_attachments" to listOf("origin" to "TEXT NOT NULL DEFAULT 'upload'"),
            // 提交来源（T3，2026-11-09）：桌面 schema v14 给 `capture_runs` 加了两列。
            // ⚠ 顺序照桌面那份（`source` 之后、`prompt_id` 之前），`HarnessDatabaseTest`
            //   会逐列比对两端 —— 漏掉这里就是"手机镜像少两列"，而同步是整行覆盖的，
            //   少了列的表现是那条捕获记录在手机上少一块内容（不报错）。
            "capture_runs" to listOf(
                "source_kind" to "TEXT NOT NULL DEFAULT 'ComfyUI'",
                "recipe_id" to "INTEGER NULL",
            ),
        )

        private fun userVersion(session: SqlSession): Int =
            session.queryOne("PRAGMA user_version")?.int("user_version") ?: 0
    }
}
