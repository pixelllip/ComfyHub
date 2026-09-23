package com.comfyhub.harness.store

/**
 * 每条连接都要走的 PRAGMA（🔒 `docs/sqlite-schema-contract.md` §2）。
 *
 * **R1 是这里唯一真正要命的一条**：`PRAGMA foreign_keys` 默认 **OFF**、而且**按连接生效** ——
 * 漏了不会报错，只是外键静默失效（删会话不级联、删标签留一堆孤儿关联行）。
 * 所以本文件提供两件事：
 *  1. [apply]：连接工厂唯一入口，任何开连接的地方都走它；
 *  2. [problems]：**读回来自检**（"写了就算"不算数 —— 连接池里另一条连接可能没带上）。
 */
object MobilePragmas {

    /**
     * ⚠ `journal_mode = WAL` 是**持久化在文件头**的（设一次即可），其余三条按连接。
     * 仍然每次都执行：幂等，而且省得区分"这条连接是不是第一条"。
     */
    val STATEMENTS: List<String> = listOf(
        "PRAGMA journal_mode = WAL",
        "PRAGMA synchronous  = NORMAL",
        "PRAGMA busy_timeout = 5000",
        "PRAGMA foreign_keys = ON",
    )

    fun apply(session: SqlSession) {
        STATEMENTS.forEach { session.exec(it) }
    }

    /**
     * 启动自检：把该为真的东西**读回来**核对，返回问题清单（空 = 通过）。
     *
     * 两个检查各自对应一条红线：
     *  - `foreign_keys` 必须是 **1**（**R1**）；
     *  - FTS5 必须可用（**R14**）—— 系统 SQLite（Android 13 的 3.32a）没有 FTS5，
     *    真用了系统库的话这一步会失败，而失败要**在启动时**炸，不能等到用户搜索时才发现。
     */
    fun problems(session: SqlSession): List<String> {
        val out = mutableListOf<String>()

        val fk = session.queryOne("PRAGMA foreign_keys")?.long("foreign_keys")
        if (fk != 1L) out += "R1 未生效：PRAGMA foreign_keys=$fk（应为 1）—— 外键会静默失效"

        val fts = runCatching {
            session.exec("CREATE VIRTUAL TABLE IF NOT EXISTS temp.fts_probe USING fts5(x)")
            session.exec("DROP TABLE IF EXISTS temp.fts_probe")
        }
        if (fts.isFailure) {
            out += "R14 未生效：FTS5 不可用（${fts.exceptionOrNull()?.message}）—— 多半用到了系统 SQLite"
        }
        return out
    }
}
