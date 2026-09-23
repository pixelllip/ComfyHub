package com.comfyhub

/**
 * 搬迁工具的命令行入口：`pwsh -File scripts\server.ps1 migrate`（Gradle 任务 `migrateData`）。
 *
 * ```
 * 默认来源：jdbc:mysql://127.0.0.1:3307/comfy_hub（本机便携版 MySQL）
 * 目标库  ：COMFYHUB_JDBC_URL，缺省就是 <项目根>\.db\comfyhub.db（P2 起的默认库）
 * ```
 *
 * 环境变量：`COMFYHUB_MIGRATE_SOURCE`（来源 URL）、`COMFYHUB_MIGRATE_USER` /
 * `COMFYHUB_MIGRATE_PASSWORD`（缺省复用 `COMFYHUB_DB_USER` / `COMFYHUB_DB_PASSWORD`）。
 * 命令行开关：`--force`（目标库非空时先清空）、`--events`（连 `ai_run_events` 一起搬）。
 *
 * ⚠ **不能让 `COMFYHUB_JDBC_URL` 指到 MySQL** —— 它同时也是"目标库"的来源。
 * 来源请走 `COMFYHUB_MIGRATE_SOURCE`。
 */
fun main(args: Array<String>) {
    val env = System.getenv()
    val force = args.any { it == "--force" } || env["COMFYHUB_MIGRATE_FORCE"] == "1"
    val events = args.any { it == "--events" } || env["COMFYHUB_MIGRATE_EVENTS"] == "1"

    val source = args.firstOrNull { it.startsWith("--source=") }?.removePrefix("--source=")
        ?: env["COMFYHUB_MIGRATE_SOURCE"]?.takeIf { it.isNotBlank() }
        ?: ("jdbc:mysql://127.0.0.1:3307/comfy_hub" +
            "?useUnicode=true&characterEncoding=UTF-8&allowPublicKeyRetrieval=true&useSSL=false")
    val user = env["COMFYHUB_MIGRATE_USER"]?.takeIf { it.isNotBlank() }
        ?: env["COMFYHUB_DB_USER"]?.takeIf { it.isNotBlank() } ?: "comfyhub"
    val password = env["COMFYHUB_MIGRATE_PASSWORD"]?.takeIf { it.isNotBlank() }
        ?: env["COMFYHUB_DB_PASSWORD"]?.takeIf { it.isNotBlank() } ?: "comfyhub"

    val target = AppConfig.fromEnv()
    check(target.dbDialect == DbDialect.SQLITE) {
        "目标库不是 SQLite（COMFYHUB_JDBC_URL=${target.jdbcUrl}）。" +
            "搬迁只往 SQLite 写；来源请用 COMFYHUB_MIGRATE_SOURCE 指定。"
    }
    target.ensureStorage()

    println("==> 搬迁开始")
    println("    来源: $source")
    println("    目标: ${target.jdbcUrl}")
    if (events) println("    含 ai_run_events（默认丢弃，见总纲 §7 待定项 5）")

    Db.init(target)
    try {
        Migrate.run()
        val report = MigrateData.run(
            sourceJdbcUrl = source,
            sourceUser = user,
            sourcePassword = password,
            target = target,
            includeEvents = events,
            force = force,
        )

        println()
        println("%-18s %10s %10s  %s".format("表", "来源行数", "目标行数", "核对"))
        report.tables.forEach { r ->
            val mark = when {
                r.skipped -> "跳过（${r.note}）"
                r.matched -> "OK  (${r.hash.take(12)})"
                else -> "**不一致**"
            }
            println("%-18s %10d %10d  %s".format(r.table, r.sourceRows, r.targetRows, mark))
        }
        println()
        println("合计 ${report.totalRows} 行；结果：${if (report.ok) "全部一致 ✅" else "有不一致 ❌"}")
        if (!report.ok) {
            kotlin.system.exitProcess(2)
        }
    } catch (e: Exception) {
        System.err.println("搬迁失败：${e.message}")
        e.printStackTrace()
        kotlin.system.exitProcess(1)
    } finally {
        Db.close()
    }
}
