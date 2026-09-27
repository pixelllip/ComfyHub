package com.comfyhub.harness.sync

import com.comfyhub.harness.store.HarnessStore
import com.comfyhub.harness.store.MirrorApply
import com.comfyhub.harness.store.MobileSchema
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.longOrNull

/**
 * **下行通道的落库那一半**（契约 🔒 `docs/sync-api-contract.md` §2 / §3）：
 * 把 `/api/sync/bootstrap` 的一页、或 `/api/sync/changes` 的一批，
 * **按镜像表顺序整行覆盖**进本地库。
 *
 * ```
 * bootstrap: {table, page, pageSize, totalPages, rows:[完整行]}
 * changes:   {upserts:{表:[完整行]}, deletes:{表:[主键]}}
 * ```
 *
 * # 职责边界：本类只做"JSON → 库"
 *
 * 取 manifest / 带游标 / 判 `hasMore` / 认 `410 Gone` 那些是**通道**的事
 * （`SyncPush` 是上行那一半的写法参照）。这里只回答一个问题：
 * **这一批 JSON 怎么落进本地镜像，而且不许把用户删掉的东西弄回来**。
 * 分层成"JSON → 库"的直接好处是这一整条判据能在宿主 JVM 上用真 SQLite 测出来。
 *
 * # ★ 本机删除标记是这一层的**第一判据**（[HarnessStore.applyMirrorRow] 的三条判据）
 *
 * 手机上会话的真源是**本地库**，而用户主动删会话时上行**没有任何 delete op**（R3：只有
 * upsert）—— 桌面那份不会知道，下一次下行就会把它整行覆盖回来、**当场复活**
 * （`docs/implementation-master-plan.md` §25.1 第三条要求的那个形状 = `local_deletions`）。
 * 所以"跳过被标记的 id"这件事必须在**这一层**做，而不是在调用方：
 * 漏一处的表现是"删掉的会话过一会儿自己回来了"，两端都不报错。
 *
 * # 一次 apply 一个实例
 *
 * 实例里带着**本次批次**的父行索引（`ai_messages` → 会话、`ai_runs` → 会话）：
 * `ai_message_parts` / `ai_tool_calls` 自己不带 `conversation_id`，而它们的父行可能刚被本批
 * 落库、也可能**被删除标记跳过了** —— 后一种情况下本机根本查不到父行，
 * 只能靠同批的父行把会话 id 带下来（否则那条子行会以"父行不在"的身份去撞外键）。
 */
class SyncApply(private val store: HarnessStore) {

    /** 本次批次里 `ai_messages.id → conversation_id`（父行被跳过时也要记 —— 它正是判据的输入）。 */
    private val conversationOfMessage = HashMap<String, String>()

    /** 本次批次里 `ai_runs.id → conversation_id`（理由同上）。 */
    private val conversationOfRun = HashMap<String, String>()

    /**
     * `/api/sync/bootstrap` 的一页：**按父表在前的顺序**逐行落库
     * （[MobileSchema.MIRROR_TABLES] 的顺序就是契约里的 apply 顺序）。
     *
     * @param table 契约 §3 必填的那个 `table`（不认识的表如实计入 [ApplyOutcome.skippedUnknown]，
     *   不抛 —— 桌面加了张新表明天我们才升 schema，不该让整页作废）。
     */
    fun applyBootstrapPage(table: String, rows: List<JsonObject>): ApplyOutcome {
        var written = 0
        var skippedDeleted = 0
        var skippedUnknown = 0
        rows.forEach { row ->
            when (applyRow(table, row)) {
                MirrorApply.WRITTEN -> written++
                MirrorApply.SKIPPED_DELETED -> skippedDeleted++
                MirrorApply.UNKNOWN_TABLE -> skippedUnknown++
            }
        }
        return ApplyOutcome(written = written, skippedDeleted = skippedDeleted, skippedUnknown = skippedUnknown)
    }

    /**
     * `/api/sync/changes` 的一批：`upserts` 按 [MobileSchema.MIRROR_TABLES] 的顺序落
     * （父表在前 —— 乱序会让子行先到、当场撞外键），`deletes` 再逐表删。
     *
     * ## 下行的 delete 是**真删**，与上行的"不许发明 delete"不冲突
     *
     * 契约 §4 的 R3 说的是**手机不许往上推**一个删除（那等于替桌面做主）；
     * 而 `deletes` 是**桌面说"这行没了"** —— 手机这边照做，否则本地镜像会永远留着幽灵行
     * （契约 §2 的硬约束原话：`deletes` 必须独立成组，混进 upserts 里客户端永远收不到删除）。
     *
     * ⚠ 被本机标删的 id 收到下行 delete 时是 **no-op**（本地那行早就没了），
     * 标记**不清** —— id 是 UUID 永不复用，"哪天可以清"这个问题没有安全判据。
     */
    fun applyChanges(
        upserts: Map<String, List<JsonObject>> = emptyMap(),
        deletes: Map<String, List<JsonElement>> = emptyMap(),
    ): ApplyOutcome {
        var written = 0
        var skippedDeleted = 0
        var skippedUnknown = 0
        var deleted = 0

        // 顺序：按镜像表清单走（父表在前），而不是按 JSON 对象里的键序（那是随机的）
        MobileSchema.MIRROR_TABLES.forEach { mirror ->
            upserts[mirror.name]?.forEach { row ->
                when (applyRow(mirror.name, row)) {
                    MirrorApply.WRITTEN -> written++
                    MirrorApply.SKIPPED_DELETED -> skippedDeleted++
                    MirrorApply.UNKNOWN_TABLE -> skippedUnknown++
                }
            }
        }
        // 清单之外（桌面加了张我们还不认识的表）：如实计入，别装作没看见
        upserts.filterKeys { MobileSchema.mirrorTable(it) == null }.values.forEach { skippedUnknown += it.size }

        deletes.forEach { (table, keys) ->
            if (MobileSchema.mirrorTable(table) == null) {
                skippedUnknown += keys.size
                return@forEach
            }
            keys.forEach { key -> if (deleteOne(table, key)) deleted++ }
        }
        return ApplyOutcome(
            written = written,
            deleted = deleted,
            skippedDeleted = skippedDeleted,
            skippedUnknown = skippedUnknown,
        )
    }

    /**
     * 单行入口（bootstrap / changes 之外，用例与将来"单条下行"都用它）。
     *
     * 它做的事就是两件：把**同批父行索引**更新好，再交给 [HarnessStore.applyMirrorRow]
     * （判据都在那边 —— 这一层不该有第二份"什么算被删过"的实现）。
     */
    fun applyRow(table: String, row: JsonObject): MirrorApply {
        val values = row.toRowValues()

        // ① 父行索引：**无论这一行会不会被落库都要记**。被删除标记跳过的那条父行
        //    恰恰是子行最需要的那条信息（"我妈那个会话被删了 ⇒ 我也别进去"）。
        val conversationId = values["conversation_id"]?.toString()?.takeIf { it.isNotBlank() }
        if (conversationId != null) {
            when (table) {
                "ai_messages" -> values["id"]?.toString()?.let { conversationOfMessage[it] = conversationId }
                "ai_runs" -> values["id"]?.toString()?.let { conversationOfRun[it] = conversationId }
            }
        }

        // ② 子行带上"父行说的那个会话"（本机可能已经查不到父行了）
        val hint = when (table) {
            "ai_message_parts" -> values["message_id"]?.toString()?.let { conversationOfMessage[it] }
            "ai_tool_calls" -> values["run_id"]?.toString()?.let { conversationOfRun[it] }
            else -> null
        }
        return store.applyMirrorRow(table, values, hint)
    }

    /** `deletes` 里的一项 → 主键列表（单列主键是标量，复合主键是数组）。 */
    private fun deleteOne(table: String, key: JsonElement): Boolean {
        val mirror = MobileSchema.mirrorTable(table) ?: return false
        val keys: List<Any?> = when (mirror.pkKind) {
            MobileSchema.PkKind.SINGLE -> listOf(key.toScalar())
            MobileSchema.PkKind.COMPOSITE -> (key as? JsonArray)?.map { it.toScalar() } ?: return false
        }
        if (keys.size != mirror.pkColumns.size) return false
        return store.deleteMirrorRow(table, keys)
    }
}

/** 一次下行 apply 的计数（给人看的那种"落了几行、跳过几行"）。 */
data class ApplyOutcome(
    val written: Int = 0,
    val deleted: Int = 0,
    /** 被**本机删除标记**跳过的行数（正常且必须 —— 用户在这台手机上删过它们）。 */
    val skippedDeleted: Int = 0,
    /** 不认识的表 / 列型（桌面比我们新）而跳过的行数。 */
    val skippedUnknown: Int = 0,
)

// ---------------------------------------------------------------------------
//  JSON → 库值
// ---------------------------------------------------------------------------

/**
 * 一行 JSON → `列名 → 值`（值只取 SQLite 真有的那几类）。
 *
 * 三个细节都是"错了不会报错、只会静默丢东西"的那种：
 *  - **JSON null → SQL NULL**（不是字符串 "null"）；
 *  - `true` / `false` → 布尔（由 `SqlSession` 的 `param` 折成 0/1：SQLite 没有布尔类型）；
 *  - 数字：**整数值给 Long**（`1.0` 也给 Long，否则 `seq = 1.0` 与 `seq = 1` 在 SQLite 里
 *    是两个不同的值，而比较查询用的是后者）；
 *  - 其余一律按**原文**给字符串（`JsonPrimitive.content`，不引号、不转义）。
 */
private fun JsonObject.toRowValues(): Map<String, Any?> = mapValues { (_, value) -> value.toScalar() }

private fun JsonElement.toScalar(): Any? = when (this) {
    is JsonNull -> null
    is JsonObject, is JsonArray -> toString()
    is JsonPrimitive -> when {
        isString -> content
        content == "true" -> true
        content == "false" -> false
        else -> longOrNull ?: doubleOrNull ?: booleanOrNull ?: content
    }
}
