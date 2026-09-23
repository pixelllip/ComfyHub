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
            if (current < MobileSchema.VERSION) {
                // 目前只有 v1；将来加列/加表时在这里按 current 逐级补（幂等写法，见 Migrate.kt）
                session.exec("PRAGMA user_version = ${MobileSchema.VERSION}")
            }
            return HarnessDatabase(session)
        }

        private fun userVersion(session: SqlSession): Int =
            session.queryOne("PRAGMA user_version")?.int("user_version") ?: 0
    }
}
