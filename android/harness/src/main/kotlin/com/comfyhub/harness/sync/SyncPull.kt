package com.comfyhub.harness.sync

import com.comfyhub.harness.net.HttpFailure
import com.comfyhub.harness.net.HttpTransport
import com.comfyhub.harness.store.HarnessStore
import com.comfyhub.harness.store.MobileSchema
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * **下行通道**（契约 🔒 `docs/sync-api-contract.md` §1~§3）：把桌面的那份投影到本地镜像。
 *
 * ```
 * GET /api/sync/manifest                          → {schemaVersion, kitVersion, cursor, serverTime, tables}
 * GET /api/sync/changes?since=<游标>&limit=500    → {cursor, hasMore, upserts:{表:[整行]}, deletes:{表:[主键]}}
 * GET /api/sync/bootstrap?table=<表>&page=N&pageSize=200
 *                                                 → {table, page, pageSize, totalPages, rows:[整行]}
 * ```
 *
 * # 为什么是这两条路（判据都来自契约，不是猜的）
 *
 * | 情形 | 走哪条 | 契约原话 |
 * | --- | --- | --- |
 * | `schemaVersion` 与本机不一致 | **bootstrap** | §1："与手机不一致时手机必须先走 bootstrap（结构对不上，增量一定错）" |
 * | 本机游标是 0（从没同步过 / 镜像刚被丢弃） | **bootstrap** | §3："首次全量（手机刚装上 / schemaVersion 变了 / 收到 410）" |
 * | 其余 | **changes** | §2 增量 |
 * | `changes` 回 **410 Gone** | 丢弃镜像 + **bootstrap** | §2："游标落在窗口之外 → 410 Gone"，§5 的裁剪与它是一对 |
 *
 * # 三条不能破的纪律
 *
 * 1. **游标只在"整趟成功"之后才写**。中途网络失败时**一个字都不许动** ——
 *    游标一旦跳到一个"还没拉完"的位置，中间那些变更就**永远补不回来**了
 *    （与上行"只有 `applied` 才清 outbox"是同一条纪律的两个方向）。
 *    已经落库的行不需要回滚：整行覆盖是**幂等**的，下次再拉一遍结果相同（契约 §2）。
 * 2. **`hasMore` 必须继续拉**（§2）：不能只看第一页就更新游标 —— 那正是丢变更的经典写法。
 * 3. **bootstrap 的 `table` 必填**（§3 的实现补充）：外层循环是"按镜像表清单逐表拉到底"，
 *    `totalPages` 才自洽；**顺序就是 [MobileSchema.MIRROR_TABLES] 的顺序**（父表在前，
 *    否则子行先到会撞外键，R1 的 `foreign_keys` 是开着的）。
 *
 * # 本类只做 HTTP + 编排
 *
 * 落库 / 跳过判据（含**本机删除标记**）全在 [SyncApply] + `HarnessStore.applyMirrorRow`；
 * 这样"删掉的会话会不会复活"那条判据只有一份实现（`SyncApplyTest` 盯着它）。
 */
class SyncPullClient(
    private val store: HarnessStore,
    private val transport: HttpTransport,
) {

    /**
     * 拉一次（manifest → 决定 bootstrap 还是 changes → 落库 → 推进游标）。
     *
     * **不抛异常**：网络 / 回执读不懂都折成 [PullOutcome.ok] = false + `error`，
     * 并把**未改动的**游标一起回出去（调用方据此如实说"这次没同步成，下次接着拉"）。
     *
     * @param maxRounds `hasMore` 连续为真的轮数上限。**必须有**：桌面回执异常时
     *   （比如游标不前进）这里不能变成一个死循环，那是"手机上同步转圈转到没电"。
     */
    fun pullOnce(
        baseUrl: String,
        token: String? = null,
        pageSize: Int = DEFAULT_PAGE_SIZE,
        limit: Int = DEFAULT_CHANGES_LIMIT,
        maxRounds: Int = MAX_ROUNDS,
    ): PullOutcome {
        val base = baseUrl.trim().trimEnd('/')
        if (base.isEmpty()) {
            return PullOutcome(
                ok = false,
                mode = MODE_UP_TO_DATE,
                cursor = store.syncCursor(),
                error = "还不知道桌面地址（本机没有连过桌面）",
            )
        }
        val headers = buildMap {
            if (!token.isNullOrBlank()) put("X-ComfyHub-Token", token)
        }

        // ① manifest：握手。读不懂 / 拿不到就**一条都不动**（游标原样回出去）
        val fetched = fetchManifest(base, headers)
        val manifest = fetched.manifest
            ?: return PullOutcome(
                ok = false,
                mode = MODE_UP_TO_DATE,
                cursor = store.syncCursor(),
                error = fetched.error ?: "拿不到桌面的同步清单（/api/sync/manifest）：这次不同步，本机游标没动",
            )

        val localCursor = store.syncCursor()
        val schemaMismatch = manifest.schemaVersion != MobileSchema.VERSION
        var mirrorDropped = 0

        if (schemaMismatch) {
            // 结构都对不上了 ⇒ 增量一定错（契约 §1），镜像（是在旧结构下落的）也不能信：
            // 丢弃它再全量，得到的就是**桌面说了算**的一份干净投影。
            // ⚠ 丢弃的判据仍由 `_outbox` 那条纪律守着：本机独有的（还没推上去的）一行不丢。
            mirrorDropped = store.resetMirror()
        }

        if (localCursor > 0 && !schemaMismatch) {
            when (val attempt = pullChanges(
                base, headers,
                since = localCursor, limit = limit, maxRounds = maxRounds,
            )) {
                is ChangesAttempt.Done -> return attempt.outcome
                // 410：镜像刚被丢弃（丢弃的条数一起带出去，如实告诉用户"重建了多少"）
                is ChangesAttempt.Gone -> mirrorDropped = attempt.mirrorDropped
            }
        }

        // ② 全量（首次 / schema 变了 / 刚收到 410）
        return bootstrap(base, headers, manifest, pageSize, mirrorDropped)
    }

    // -----------------------------------------------------------------------
    //  增量
    // -----------------------------------------------------------------------

    /** 增量那一趟的结果：要么有个确定结论，要么"收到 410，改走全量"。 */
    private sealed interface ChangesAttempt {
        data class Done(val outcome: PullOutcome) : ChangesAttempt

        /** 契约 §2/§5：游标落在保留窗口之外 → 镜像已丢弃，必须走全量。 */
        data class Gone(val mirrorDropped: Int) : ChangesAttempt
    }

    /**
     * 增量拉取。
     *
     * ⚠ 一路上**一次都不写游标**（只在整趟成功、`hasMore` 走完之后写一次）：
     * 中途失败时游标一个字都不许动，否则中间那些变更**永远补不回来**。
     * 已经落库的行不需要回滚 —— 整行覆盖是幂等的（契约 §2），下次再拉一遍结果相同。
     */
    private fun pullChanges(
        base: String,
        headers: Map<String, String>,
        since: Long,
        limit: Int,
        maxRounds: Int,
    ): ChangesAttempt {
        // 一次 pull 一个 apply 实例：父行索引要跨轮有效 —— `hasMore` 会把父子行切到两页里，
        // 而 `ai_message_parts` / `ai_tool_calls` 自己不带 `conversation_id`。
        val apply = SyncApply(store)
        var written = 0
        var deleted = 0
        var skippedDeleted = 0
        var skippedUnknown = 0
        var cursor = since
        var round = 0

        while (true) {
            if (round >= maxRounds) {
                return ChangesAttempt.Done(
                    failed(
                        MODE_CHANGES,
                        written + deleted,
                        "增量连续 $maxRounds 轮都还有下一页（桌面回执异常？）；这次不推进游标",
                        skippedDeleted,
                        skippedUnknown,
                    ),
                )
            }
            val reply = try {
                transport.get("$base/api/sync/changes?since=$cursor&limit=$limit", headers)
            } catch (e: HttpFailure) {
                return ChangesAttempt.Done(
                    failed(MODE_CHANGES, written + deleted, e.message.orEmpty(), skippedDeleted, skippedUnknown),
                )
            }
            if (reply.status == HTTP_GONE) {
                // ★ 契约 §2/§5：游标落在保留窗口之外 → 丢弃镜像再全量。
                //   丢完游标置 0，所以即使紧接着的全量半路失败，下次也只会走全量。
                return ChangesAttempt.Gone(store.resetMirror())
            }
            if (!reply.ok) {
                return ChangesAttempt.Done(
                    failed(
                        MODE_CHANGES,
                        written + deleted,
                        "拉增量失败：HTTP ${reply.status} ${reply.body.take(200)}",
                        skippedDeleted,
                        skippedUnknown,
                    ),
                )
            }
            val root = parseObject(reply.body)
                ?: return ChangesAttempt.Done(
                    failed(MODE_CHANGES, written + deleted, "拉增量失败：回执读不懂", skippedDeleted, skippedUnknown),
                )
            val nextCursor = root.long("cursor") ?: cursor
            val hasMore = root.bool("hasMore") ?: false
            val upserts = upsertsOf(root)
                ?: return ChangesAttempt.Done(
                    failed(
                        MODE_CHANGES,
                        written + deleted,
                        "拉增量失败：回执里有一行不是 JSON 对象",
                        skippedDeleted,
                        skippedUnknown,
                    ),
                )
            val outcome = apply.applyChanges(
                upserts = upserts,
                deletes = deletesOf(root),
            )
            written += outcome.written
            deleted += outcome.deleted
            skippedDeleted += outcome.skippedDeleted
            skippedUnknown += outcome.skippedUnknown

            if (hasMore && nextCursor <= cursor) {
                // 游标没前进 + 还有下一页 = 死循环的入口。如实报错，**不推进游标**（正确性优先）
                return ChangesAttempt.Done(
                    failed(
                        MODE_CHANGES,
                        written + deleted,
                        "桌面回的游标没有前进（$cursor → $nextCursor）却还说 hasMore；这次不推进游标",
                        skippedDeleted,
                        skippedUnknown,
                    ),
                )
            }
            cursor = nextCursor
            if (!hasMore) break
            round++
        }

        // 整趟成功：这才写游标（中途任何失败都不写，见类注释第 1 条）
        store.setSyncCursor(cursor)
        return ChangesAttempt.Done(
            PullOutcome(
                ok = true,
                mode = if (written + deleted == 0) MODE_UP_TO_DATE else MODE_CHANGES,
                applied = written + deleted,
                cursor = cursor,
                skippedDeleted = skippedDeleted,
                skippedUnknown = skippedUnknown,
            ),
        )
    }

    // -----------------------------------------------------------------------
    //  全量
    // -----------------------------------------------------------------------

    /**
     * 全量（bootstrap）：按 [MobileSchema.MIRROR_TABLES] 的顺序**逐表拉到底**，最后把游标
     * 置成 `manifest.cursor`（契约 §3 原话："**不是** bootstrap 自己的页号"）。
     *
     * ⚠ 用**一个** [SyncApply] 实例跑完整趟：父子行可能被分页切开（200 行一页），
     * 父行索引必须跨页有效。
     */
    private fun bootstrap(
        base: String,
        headers: Map<String, String>,
        manifest: Manifest,
        pageSize: Int,
        /** 这一趟之前丢弃了多少行镜像（410 / schema 变了）：如实带出去给用户看。 */
        mirrorDropped: Int = 0,
    ): PullOutcome {
        val apply = SyncApply(store)
        var written = 0
        var skippedDeleted = 0
        var skippedUnknown = 0

        MobileSchema.MIRROR_TABLES.forEach { mirror ->
            var page = 1
            var totalPages = 1
            do {
                val reply = try {
                    transport.get(
                        "$base/api/sync/bootstrap?table=${mirror.name}&page=$page&pageSize=$pageSize",
                        headers,
                    )
                } catch (e: HttpFailure) {
                    return failed(
                        MODE_BOOTSTRAP, written, e.message.orEmpty(),
                        skippedDeleted, skippedUnknown, mirrorDropped,
                    )
                }
                if (!reply.ok) {
                    return failed(
                        MODE_BOOTSTRAP,
                        written,
                        "拉全量失败（${mirror.name} 第 $page 页）：HTTP ${reply.status} ${reply.body.take(200)}",
                        skippedDeleted,
                        skippedUnknown,
                        mirrorDropped,
                    )
                }
                val root = parseObject(reply.body)
                    ?: return failed(
                        MODE_BOOTSTRAP, written, "拉全量失败：回执读不懂",
                        skippedDeleted, skippedUnknown, mirrorDropped,
                    )
                val rows = (root["rows"] as? JsonArray)?.toList().orEmpty()
                val objects = rows.mapNotNull { it as? JsonObject }
                if (objects.size != rows.size) {
                    // 静默丢一行 = 镜像里永远缺那一行，而两端都不报错。宁可这次不成。
                    return failed(
                        MODE_BOOTSTRAP,
                        written,
                        "全量回执里有一行不是 JSON 对象（${mirror.name} 第 $page 页）",
                        skippedDeleted,
                        skippedUnknown,
                        mirrorDropped,
                    )
                }
                val outcome = apply.applyBootstrapPage(mirror.name, objects)
                written += outcome.written
                skippedDeleted += outcome.skippedDeleted
                skippedUnknown += outcome.skippedUnknown
                // `totalPages` 最小值是 1（空表也是 1，见桌面 `bootstrap` 的实现）
                totalPages = (root.long("totalPages") ?: 1L).coerceAtLeast(1L).toInt()
                page++
            } while (page <= totalPages)
        }

        store.setSyncCursor(manifest.cursor)
        return PullOutcome(
            ok = true,
            mode = MODE_BOOTSTRAP,
            applied = written,
            cursor = manifest.cursor,
            skippedDeleted = skippedDeleted,
            skippedUnknown = skippedUnknown,
            mirrorDropped = mirrorDropped,
        )
    }

    // -----------------------------------------------------------------------
    //  manifest
    // -----------------------------------------------------------------------

    private data class Manifest(val schemaVersion: Int, val cursor: Long, val kitVersion: String?)

    /** 握手的结果：要么拿到清单，要么带着**人话的原因**（网络 / 状态码 / 读不懂）回来。 */
    private data class ManifestFetch(val manifest: Manifest?, val error: String?)

    private fun fetchManifest(base: String, headers: Map<String, String>): ManifestFetch {
        val reply = try {
            transport.get("$base/api/sync/manifest", headers)
        } catch (e: HttpFailure) {
            return ManifestFetch(null, "拿不到桌面的同步清单（${e.message}）：这次不同步，本机游标没动")
        }
        if (!reply.ok) {
            return ManifestFetch(
                null,
                "拿不到桌面的同步清单（HTTP ${reply.status}）：这次不同步，本机游标没动",
            )
        }
        val root = parseObject(reply.body)
            ?: return ManifestFetch(null, "桌面回的同步清单读不懂：这次不同步，本机游标没动")
        // `schemaVersion` / `cursor` 缺了就当"读不懂"（缺了还要继续，等于闭着眼同步）
        val schemaVersion = root.int("schemaVersion")
            ?: return ManifestFetch(null, "桌面回的同步清单缺 schemaVersion：这次不同步")
        val cursor = root.long("cursor")
            ?: return ManifestFetch(null, "桌面回的同步清单缺 cursor：这次不同步")
        return ManifestFetch(Manifest(schemaVersion, cursor, root.str("kitVersion")), null)
    }

    // -----------------------------------------------------------------------
    //  解析小工具
    // -----------------------------------------------------------------------

    private fun upsertsOf(root: JsonObject): Map<String, List<JsonObject>>? =
        (root["upserts"] as? JsonObject).orEmpty().entries.associate { (table, value) ->
            val rows = (value as? JsonArray)?.toList().orEmpty()
            val objects = rows.mapNotNull { it as? JsonObject }
            // 不是对象的行**不许静默丢**（丢一行 = 镜像里永远缺那一行）：回 null 让调用方如实报错
            if (objects.size != rows.size) return null
            table to objects
        }

    private fun deletesOf(root: JsonObject): Map<String, List<JsonElement>> =
        (root["deletes"] as? JsonObject).orEmpty()
            .mapNotNull { (table, value) -> (value as? JsonArray)?.let { table to it.toList() } }
            .toMap()

    private fun failed(
        mode: String,
        applied: Int,
        error: String,
        skippedDeleted: Int = 0,
        skippedUnknown: Int = 0,
        mirrorDropped: Int = 0,
    ): PullOutcome = PullOutcome(
        ok = false,
        mode = mode,
        applied = applied,
        // ⚠ 失败时回的是**库里那个（没动过的）游标**，不是"我们本想推到哪"
        cursor = store.syncCursor(),
        skippedDeleted = skippedDeleted,
        skippedUnknown = skippedUnknown,
        mirrorDropped = mirrorDropped,
        error = error,
    )

    companion object {
        /** 契约 §3 的实现补充：`pageSize` 缺省 200（桌面自己也钳在 [1, 1000]）。 */
        const val DEFAULT_PAGE_SIZE = 200

        /** 契约 §2：`limit` 缺省 500（桌面钳在 [1, 500]）。 */
        const val DEFAULT_CHANGES_LIMIT = 500

        /**
         * `hasMore` 连续为真的轮数上限。桌面一批最多 500 行，正常同步不可能连着几十轮；
         * 撞到这个数说明回执有问题，**如实报错比死循环好**。
         */
        const val MAX_ROUNDS = 50

        const val MODE_BOOTSTRAP = "bootstrap"
        const val MODE_CHANGES = "changes"
        const val MODE_UP_TO_DATE = "up_to_date"

        /** 契约 §2：游标落在保留窗口之外。手机据此丢弃镜像并改走 bootstrap。 */
        const val HTTP_GONE = 410
    }
}

/**
 * 一次下行同步的结果（形状：`{ok, mode, applied, cursor, error?}`）。
 *
 * [mode] 是**这次实际走的那条路**（或"本来打算走哪条"）：[SyncPullClient.MODE_BOOTSTRAP] /
 * [SyncPullClient.MODE_CHANGES] / [SyncPullClient.MODE_UP_TO_DATE]。
 *
 * ⚠ `ok = false` 时**别拿 [mode] 当结论**（判据永远是 `ok`）：握手就失败时我们还没选路，
 * 那时它只是 `up_to_date` 这个"什么都没做"的默认值。
 */
data class PullOutcome(
    val ok: Boolean,
    val mode: String,
    /** 落库的行数（upsert + delete；被本机删除标记跳过的不算）。 */
    val applied: Int = 0,
    /** **成功之后**的游标；失败时是库里那个没动过的值。 */
    val cursor: Long = 0,
    /** 被**本机删除标记**跳过的行数（正常且必须：用户在这台手机上删过它们）。 */
    val skippedDeleted: Int = 0,
    /** 不认识的表 / 行而跳过的条数（桌面比我们新）。 */
    val skippedUnknown: Int = 0,
    /**
     * 这一趟为了重建镜像**丢弃**的行数（410 / `schemaVersion` 变了时才有；其余恒 0）。
     *
     * 它不是"新增的必须字段"，而是**如实**：用户该知道"本机镜像被重建过一次"
     * （丢掉的都是桌面那份的投影 —— `_outbox` 里还有待推行的那些行一行都没丢）。
     */
    val mirrorDropped: Int = 0,
    val error: String? = null,
)

// ---------------------------------------------------------------------------
//  JSON 取值（**读不懂就回 null**，由调用方决定那是"失败"还是"用默认值"）
// ---------------------------------------------------------------------------

private fun parseObject(text: String): JsonObject? =
    runCatching { com.comfyhub.harness.protocol.ProtocolJson.parseToJsonElement(text) as? JsonObject }.getOrNull()

private fun JsonObject.str(key: String): String? =
    (this[key] as? JsonPrimitive)?.content?.takeIf { it.isNotBlank() }

private fun JsonObject.long(key: String): Long? = str(key)?.toLongOrNull()

private fun JsonObject.int(key: String): Int? = str(key)?.toIntOrNull()

private fun JsonObject.bool(key: String): Boolean? = str(key)?.toBooleanStrictOrNull()
