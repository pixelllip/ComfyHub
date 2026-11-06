//! SQLite 连接与事务纪律（🔒 `docs/sqlite-schema-contract.md` §2，红线 **R1** / **R2**）。
//!
//! 这个文件**只有一件事**：让"每一条连接都带上正确的 PRAGMA"和"写事务一律 `BEGIN IMMEDIATE`"
//! 成为**唯一入口**，而不是散落在各个 handler 里的自觉。
//!
//! 为什么值得单独一个模块（而不是在 `main.rs` 里开个连接了事）：
//!
//! - **R1**：`PRAGMA foreign_keys` 默认 **OFF**、而且**按连接生效**。漏了不报错，
//!   只是外键静默失效（删会话不级联、删标签留一堆孤儿关联行）。
//!   所以这里不仅有 `apply`，还有 [`assert_ready`]：**把值读回来核对**，
//!   不对就当场抛 —— "写了就算"不算数（连接池里另一条连接可能没带上）。
//! - **R2**：写事务一律 `BEGIN IMMEDIATE`（[`tx_immediate`]）。裸 `BEGIN` 是延迟事务，
//!   读锁升级写锁那一下**可能不理会 `busy_timeout`** 直接 `SQLITE_BUSY`。
//!
//! 与 Kotlin 侧的对应物：`Db.kt`（方言化 + PRAGMA 自检 + `BEGIN IMMEDIATE`）。

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::path::Path;

/// 一条连接的 PRAGMA 清单（顺序无关，但**一条都不能少**）。
pub const PRAGMAS: &str = "
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous  = NORMAL;
    PRAGMA busy_timeout = 5000;
    PRAGMA foreign_keys = ON;
    PRAGMA temp_store = MEMORY;
";

/// ⚠ **`temp_store = MEMORY` 不是调优，是"别依赖一个可写的 `%TEMP%`"**（2026-09-27 真机实测）。
///
/// 现场：`DELETE /api/prompts/{id}` 在**活着且健康**的后端上稳定回
/// `500 {"error":"数据库错误：unable to open database file"}` —— 而同一条删除
/// 对着**同一份库的副本**（另一个进程）却回 200。差别只有一个：
/// 那个进程的映像在工作区内、**不能在 `%TEMP%` 建文件**，而 SQLite 默认
/// （`temp_store = FILE`）会把排序/临时表溢出到 `%TEMP%` —— 建不出那个文件，
/// 于是报的是"打不开数据库文件"，看着像库坏了，其实是**临时文件建不出来**。
///
/// 放到 `PRAGMAS` 里而不是每处 `PRAGMA`：它是**连接级**的，只设一处必然漏。
/// 顺带对发布包也是好事：便携包可能落在只读/受限的目录里，别再额外依赖 `%TEMP%` 可写。
pub fn assert_temp_store_is_memory(conn: &Connection) -> rusqlite::Result<()> {
    // 2 = MEMORY（0 = 默认、1 = FILE）
    let mode: i64 = conn.query_row("PRAGMA temp_store", [], |row| row.get(0))?;
    if mode != 2 {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "temp_store={mode}（应为 2 = MEMORY）：临时表会落到 %TEMP%，\
             那里不可写时删除/排序会报成 'unable to open database file'"
        )));
    }
    Ok(())
}

/// 开一条**已经上好 PRAGMA** 的连接（连接工厂 —— 别在别处直接 `Connection::open`）。
///
/// `WAL` 允许读写并发，所以"一条写连接 + 若干只读连接"是安全的；
/// Rust 侧不引连接池（`tokio-rusqlite` 的专属写线程在端点铺开后再加）。
/// 建表用的 DDL（**编译期嵌进二进制**）。
///
/// 与 Kotlin 从 jar 里读 `db/schema.sqlite.sql` **同一性质**：真源是仓库里那份文件，
/// 二进制自带一份副本。⚠ 改了 DDL 要**重新编译后端**（仓库里 `db\schema.sqlite.sql`
/// 那份留给人和 sqlite3 CLI 手工建库用，运行期不读它）。
pub const SCHEMA_SQL: &str = include_str!("../../../db/schema.sqlite.sql");

/// schema 版本（`PRAGMA user_version`，见 🔒 `docs/sqlite-schema-contract.md`）。
/// 这个后端认识的 schema 版本。
///
/// **加表 / 改列 / 加同步触发器都要 +1**（2026-09-27 由 1 提到 2：`workflow_recipes` 的 DDL
/// 早就在 `db/schema.sqlite.sql` 里，而真库停在 1、**那张表根本不存在** —— 见
/// [`open_or_create`] 的 ⚠；同日由 2 提到 3：给 `ai_runs` / `ai_tool_calls` 补 `_sync_log`
/// 触发器 —— **不是加表但同样够不着老库**，理由写在 `db/schema.sqlite.sql` 末尾；
/// 同日由 3 提到 4：加 `submit_jobs` 表 = M5「手机排队 → 桌面真正提交」的持久账；
/// 2026-09-30 由 4 提到 5：给 `character_cards` 补 `position` / `enabled` 两列
/// （生图角色卡框架：位置 + 是否参与本次生成）——**这是第一次"加列"**，
/// 走的是下面 [`NEW_COLUMNS`] 那套"先 `PRAGMA table_info` 探测再 ALTER"。
/// 2026-10 由 5 提到 6：角色卡**只留三个字段**（名称 / 正面提示词 / 负面提示词）——
/// 加 `prompt` 列，并把老列的 `tags`（JSON 数组）拼成一行灌进去（见 [`backfill_character_card_prompt`]）；
/// `position` / `enabled` 与那一堆资料库列从此**没人再读**（老库里保留原样，新库不建）。
/// 2026-10-11 由 6 提到 7：加产物**集合**（`media_collections` 表 + `media_assets.collection_id`）——
/// 同一批、只做了一点微调的产物归到一组，详情页可左右切换（判定见 `media/collection.rs`）。
/// ⚠ 加的是**表 + 列**：老库靠"版本号 bump → 收敛式 DDL 建表" + [`NEW_COLUMNS`] 补列，
/// 两条路缺一条就是**静默的**（表不存在时产物一入库就炸 `no such table`；
/// 列不存在时集合永远是 None，界面上什么都不显示）。
/// 2026-10-11 由 7 提到 8：给 `prompts` 加 `negative_mode`（发送模式：保留 / 只发正面）。
/// 加的是**列**（走 [`NEW_COLUMNS`]）；bump 的另一个作用是让 `manifest.schemaVersion`
/// 变化，手机据此重走一次 bootstrap（提示词形状变了）。
/// **2026-10-18 由 8 提到 9：手动集合两列** ——
/// ① `media_collections.title`（用户给集合起的名字，可空）；
/// ② `media_assets.collection_opt_out`（用户**明确把这一条移出集合**了）。
///
/// ⚠ ② 不是"锦上添花的标记"，是**正确性的前提**：`backfill` 的判据是
/// `collection_id IS NULL`，而"移出集合"恰恰就是把它置成 NULL ——
/// 没有这一列，用户刚拖出去的那张会在**下次启动**被回填静默吸回原组
/// （`collection.rs` 里那句"手动拆开过的一律不碰"就只是一句空承诺）。
/// 2026-10-21 由 9 提到 10：**画风卡 + 卡片示例产物 + 卡片的双版本** ——
/// ① 新表 `style_cards`（与角色卡同形）；
/// ② 两张卡表各加 `tag_prompt`（Danbooru tag 版），`prompt` 自此专指自然语言版；
/// ③ 两张示例关联表（`character_card_examples` / `style_card_examples`）。
/// 加的是"表 + 列"，所以 bump 版本号与 `NEW_COLUMNS` 两条路都要走。
///
/// 2026-10-24 由 10 提到 11：**卡片回到单版本（只要自然语言）** ——
/// 用户口径："只保留自然语言版，tag 版删掉；老库里已经存着的 tag 版转成自然语言再删"。
/// ① 新库的 DDL 不再建 `tag_prompt`（老库那一列留在原地，**再没人读**）；
/// ② 升级到 11 的那一次把 `tag_prompt` 转成一段自然语言灌进 `prompt`
///    （见 [`migrate_card_tags_to_prompt`]）——**只填 prompt 为空的卡**，
///    已经有自然语言版的卡一个字都不动。
pub const SCHEMA_VERSION: i64 = 13;

/// **"这一列必须存在"的声明式清单**（2026-09-30 起）。
///
/// `db/schema.sqlite.sql` 是**收敛式**的，但 `CREATE TABLE IF NOT EXISTS` **不会**给已经存在的表补列 ——
/// 所以在 DDL 里加一列，老库永远拿不到它（而且不报错，表现是"这列读出来永远是默认值"）。
/// 契约（`docs/sqlite-schema-contract.md` §2）要求的就是这套写法：**先 `PRAGMA table_info` 探测再改**。
///
/// 加列的规矩：
///  1. 三处一起改：这里 + `db/schema.sqlite.sql` 的建表 DDL + [`SCHEMA_VERSION`] +1；
///  2. 只能加**可空列**或**带默认值的列**（SQLite 的 `ADD COLUMN` 不能加 `NOT NULL` 且无默认值的列）；
///  3. 别改已有列的类型 / 名字（SQLite 改不动，得重建表 —— 那种迁移要单独写，不在这个清单里）；
///  4. **删列不走这里**：这个清单只会 `ADD COLUMN`。角色卡那次收敛就是把老列留在原地
///     （SQLite 删列要重建表，而它们只是没人再读，留着不碍事）。
const NEW_COLUMNS: &[(&str, &str, &str)] = &[
    (
        "character_cards",
        "prompt",
        "ALTER TABLE character_cards ADD COLUMN prompt TEXT NULL",
    ),
    // ⚠ 2026-10-24 起**不再补 `tag_prompt`**：卡片回到单版本（只要自然语言）。
    //   老库那一列还在，由 [`migrate_card_tags_to_prompt`] 转成 prompt 之后成为死列；
    //   新库的 DDL 根本不建它。别为了"兼容"把它加回这个清单 —— 那样新库也会长出一个
    //   没人读的列，下一个人就会以为它还是真源。
    // 产物所属集合（2026-10-11）。可空 + 无默认值 → 符合"只能加可空列"的规矩。
    // 老库补上这一列之后**全是 NULL**，要靠 `collection::backfill` 那一趟才填得满。
    (
        "media_assets",
        "collection_id",
        "ALTER TABLE media_assets ADD COLUMN collection_id INTEGER NULL REFERENCES media_collections(id) ON DELETE SET NULL",
    ),
    // 发送模式（2026-10-11）：有些 turbo 模型不认负面提示词，用户要能表达"我只发正面"。
    // 带默认值（`'keep'`）→ 符合"只能加可空列或带默认值的列"的规矩；老库补上之后全是 keep。
    (
        "prompts",
        "negative_mode",
        "ALTER TABLE prompts ADD COLUMN negative_mode TEXT NOT NULL DEFAULT 'keep'",
    ),
    // 手动集合的名字（2026-10-18）。可空：算法分出来的集合没有名字。
    (
        "media_collections",
        "title",
        "ALTER TABLE media_collections ADD COLUMN title TEXT NULL",
    ),
    // 「用户已明确把这一条移出集合」（2026-10-18）。带默认值 0 → 符合加列规矩。
    // ⚠ `backfill` 的 WHERE 必须带上 `AND collection_opt_out = 0`，否则"移出"下次启动就复原。
    (
        "media_assets",
        "collection_opt_out",
        "ALTER TABLE media_assets ADD COLUMN collection_opt_out INTEGER NOT NULL DEFAULT 0",
    ),
    // 提问卡带图（2026-11-04）：按设定图逐部位确认时，题目要带裁图、选项要带示意图。
    // 两列都可空 → 符合"只能加可空列"的规矩；老库补上之后全是 NULL（旧提问一个字节不变）。
    (
        "ai_questions",
        "image_url",
        "ALTER TABLE ai_questions ADD COLUMN image_url TEXT NULL",
    ),
    (
        "ai_questions",
        "option_images_json",
        "ALTER TABLE ai_questions ADD COLUMN option_images_json TEXT NULL",
    ),
    // 附件来源（2026-11-04）：upload / crop / search。带默认值 → 符合加列规矩；
    // 老库里已有的附件一律按 upload 处理（宁可不清理，也不要把用户的图删掉）。
    (
        "ai_attachments",
        "origin",
        "ALTER TABLE ai_attachments ADD COLUMN origin TEXT NOT NULL DEFAULT 'upload'",
    ),
];

/// 给老库补上 [`NEW_COLUMNS`] 里缺的列，返回**这次真加了哪些**（`表.列`）。
///
/// 幂等：列已在就是空操作。**每次开库都跑**（两条 SQL 的代价），因为它跟 `missing_tables`
/// 一样是"版本号够新也不代表结构对"的那道自愈 —— 真机踩过一次就够记一辈子了。
///
/// ⚠ **表还不存在时直接跳过**：这时候该由 DDL 去建（它建出来就带全列），
/// 而对着一张不存在的表 `ALTER TABLE` 只会报 `no such table`。
/// 这条是"能在跑 DDL **之前**调用"的前提 —— 顺序为什么不能反见 [`open_or_create`]。
fn ensure_columns(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut added: Vec<String> = Vec::new();
    for (table, column, ddl) in NEW_COLUMNS {
        let table_exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            rusqlite::params![table],
            |row| row.get(0),
        )?;
        if table_exists == 0 {
            continue;
        }
        let present: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            rusqlite::params![table, column],
            |row| row.get(0),
        )?;
        if present == 0 {
            conn.execute_batch(ddl)?;
            added.push(format!("{table}.{column}"));
        }
    }
    Ok(added)
}

/// 这个表上有没有这一列（表不存在也是 `false`）。
fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let present: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
        rusqlite::params![table, column],
        |row| row.get(0),
    )?;
    Ok(present > 0)
}

/// **老库的数据迁移**：`character_cards.tags`（JSON 数组文本）→ `prompt`（整段文本）。
///
/// 2026-10 角色卡收敛成"名称 / 正面提示词 / 负面提示词"之后，老卡里那套规范 tag 栈
/// 就只剩 `tags` 那一列——**新代码读的是 `prompt`**，不搬过去等于把用户的卡清空了
/// （而列还在、界面上却什么都没有，最难查的一类）。所以**刚补上 `prompt` 列那一刻**
/// 把它们拼成 `a, b, c` 灌进去：迁移只加不减，宁可留得多，不许丢用户的话。
///
/// ⚠ **只在"这次真的刚补上这一列"时调**（[`open_or_create`] 用 `added` 判），不许每次开库都跑：
/// 老库那个 `tags` 列是**留在原地**的，而判据是"prompt 为空 + tags 非空"——每次开库都跑的话，
/// 用户在界面上清空提示词（`prompt = NULL`）会被下一次启动**从 tags 复活**，
/// 表现是"删了又回来"，而那种 bug 极难查。
///
/// 三条边界：
///  * **新库没有 `tags` 列** → 空操作（判据是"两个列都在"）；
///  * **`prompt` 已经有值** → 一个字节都不动（幂等；也不覆盖用户后来手填的）；
///  * `tags` 不是合法 JSON（手工改过）→ **原样当文本**用，不丢。
fn backfill_character_card_prompt(conn: &Connection) -> rusqlite::Result<usize> {
    if !column_exists(conn, "character_cards", "prompt")?
        || !column_exists(conn, "character_cards", "tags")?
    {
        return Ok(0);
    }
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, tags FROM character_cards \
             WHERE (prompt IS NULL OR prompt = '') AND tags IS NOT NULL AND tags <> ''",
        )?;
        let mapped = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut migrated = 0usize;
    for (id, raw) in rows {
        let joined = old_tags_to_prompt(&raw);
        if joined.is_empty() {
            continue;
        }
        conn.execute(
            "UPDATE character_cards SET prompt = ?1 WHERE id = ?2",
            rusqlite::params![joined, id],
        )?;
        migrated += 1;
    }
    Ok(migrated)
}

/// 老的 `tags` JSON 数组 → 一行提示词（`a, b, c`）；不是 JSON 就原样返回。
fn old_tags_to_prompt(raw: &str) -> String {
    match serde_json::from_str::<Vec<String>>(raw) {
        Ok(list) => list
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Err(_) => raw.trim().to_string(),
    }
}

/// **老库迁移（2026-10-24）**：把两张卡表里残留的 `tag_prompt` 转成自然语言灌进 `prompt`。
///
/// 用户口径：卡片只留自然语言版，tag 版不再占字段。判据是
/// "`prompt` 为空 **且** `tag_prompt` 非空" —— 已经有自然语言版的卡**一个字都不动**
/// （转出来的句式只是保底，不能盖掉用户 / AI 写好的那一段）。
///
/// 只在 `user_version < 11` 的那一次升级里跑（见 [`open_or_create`]）：老库那一列留在原地，
/// 每次开库都跑的话，用户后来**清空**提示词会被下一次启动从 tag 列复活
/// （与 [`backfill_character_card_prompt`] 同一条教训）。
fn migrate_card_tags_to_prompt(conn: &Connection) -> rusqlite::Result<usize> {
    let mut migrated = 0usize;
    for table in ["character_cards", "style_cards"] {
        if !column_exists(conn, table, "tag_prompt")? {
            continue;
        }
        let rows: Vec<(i64, String)> = {
            let mut stmt = conn.prepare(&format!(
                "SELECT id, tag_prompt FROM {table} \
                 WHERE (prompt IS NULL OR prompt = '') \
                   AND tag_prompt IS NOT NULL AND trim(tag_prompt) <> ''"
            ))?;
            let mapped = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, raw) in rows {
            let text = tags_to_natural_language(&raw);
            if text.is_empty() {
                continue;
            }
            conn.execute(
                &format!("UPDATE {table} SET prompt = ?1 WHERE id = ?2"),
                rusqlite::params![text, id],
            )?;
            migrated += 1;
        }
    }
    Ok(migrated)
}

/// 一列 Danbooru 风格 tag → 一段**尽力而为**的自然语言。
///
/// 这不是翻译器：它保留每个 tag 的信息（去下划线、按"外观 / 衣着 / 其余"分组），
/// 拼成 `A girl with red eyes and silver hair, wearing school uniform. Shinku (nte).`。
/// 迁移用它兜底；用户 / AI 之后可以用 `card-writer` 把句子重写成更顺的一版。
pub(crate) fn tags_to_natural_language(raw: &str) -> String {
    let trimmed = raw.trim();
    // 已经是整句的（没有分隔符、词数够多）**原样保留**：老库里那一列偶尔存的本来就是散文，
    // 按 tag 拆只会得到 `A character with A quiet girl with red eyes...` 这种更糟的结果。
    if !trimmed.contains(',')
        && !trimmed.contains('，')
        && !trimmed.contains(';')
        && !trimmed.contains('；')
        && !trimmed.contains('\n')
        && trimmed.split_whitespace().count() >= 8
    {
        return trimmed.to_string();
    }
    const DROP: &[&str] = &[
        "masterpiece",
        "best quality",
        "high quality",
        "highres",
        "absurdres",
        "ultra detailed",
        "8k",
        "4k",
        "score 9",
        "score 8",
        "commentary",
        "translation request",
        "english text",
        "watermark",
        "signature",
        "solo",
    ];
    const APPEARANCE: &[&str] = &[
        "hair",
        "eyes",
        "eye",
        "skin",
        "ear",
        "ears",
        "horn",
        "horns",
        "tail",
        "wings",
        "freckles",
        "mole",
        "scar",
        "tattoo",
        "breasts",
        "stomach",
        "legs",
        "arms",
        "face",
        "teeth",
        "eyelashes",
        "eyebrows",
        "nose",
        "lips",
        "navel",
        "penis",
        "vagina",
        "nipples",
        "areolae",
        "pubic",
        "thighs",
        "fingers",
        "neck",
        "height",
    ];
    const CLOTHING: &[&str] = &[
        "dress",
        "skirt",
        "shirt",
        "blouse",
        "sweater",
        "hoodie",
        "jacket",
        "coat",
        "uniform",
        "kimono",
        "yukata",
        "bikini",
        "swimsuit",
        "bra",
        "panties",
        "underwear",
        "shorts",
        "pants",
        "jeans",
        "trousers",
        "socks",
        "stockings",
        "thighhighs",
        "pantyhose",
        "gloves",
        "hat",
        "cap",
        "beret",
        "helmet",
        "ribbon",
        "bow",
        "necktie",
        "tie",
        "scarf",
        "boots",
        "shoes",
        "sandals",
        "slippers",
        "apron",
        "cape",
        "cloak",
        "armor",
        "leotard",
        "bodysuit",
        "choker",
        "necklace",
        "earrings",
        "earring",
        "glasses",
        "mask",
        "goggles",
        "bracelet",
        "ring",
        "belt",
        "suspenders",
        "collar",
    ];

    let subject_of = |lower: &str| -> Option<&'static str> {
        match lower {
            "1girl" | "girl" => Some("A girl"),
            "1boy" | "boy" => Some("A boy"),
            "2girls" => Some("Two girls"),
            "3girls" => Some("Three girls"),
            "2boys" => Some("Two boys"),
            "multiple girls" | "6+girls" => Some("Several girls"),
            "1other" => Some("A character"),
            _ => None,
        }
    };

    let mut subject: Option<&'static str> = None;
    let mut attrs: Vec<String> = Vec::new();
    let mut clothes: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();

    for raw_tag in raw.split([',', '，', ';', '；', '\n']) {
        let tag = raw_tag.replace('_', " ");
        let tag = tag.split_whitespace().collect::<Vec<_>>().join(" ");
        if tag.is_empty() {
            continue;
        }
        let lower = tag.to_lowercase();
        if let Some(found) = subject_of(&lower) {
            subject = Some(found);
            continue;
        }
        if DROP.contains(&lower.as_str()) {
            continue;
        }
        let bucket = if APPEARANCE.iter().any(|w| lower.contains(w)) {
            &mut attrs
        } else if CLOTHING.iter().any(|w| lower.contains(w)) {
            &mut clothes
        } else {
            &mut other
        };
        if !bucket.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
            bucket.push(tag);
        }
    }

    if subject.is_none() && attrs.is_empty() && clothes.is_empty() && other.is_empty() {
        return String::new();
    }
    let mut out = String::from(subject.unwrap_or("A character"));
    if !attrs.is_empty() {
        out.push_str(" with ");
        out.push_str(&join_and(&attrs));
    }
    if !clothes.is_empty() {
        out.push_str(", wearing ");
        out.push_str(&join_and(&clothes));
    }
    if !other.is_empty() {
        out.push_str(". ");
        out.push_str(&capitalize_first(&join_and(&other)));
    }
    out.push('.');
    out
}

fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// **开库（必要时建库）**：切流之后 Rust 是唯一的后端，"第一次运行要能自己建库"这件事
/// 必须在它这边成立 —— 以前那版遇到缺库直接 `exit 3`（"先让 Kotlin 起一次库"），
/// 那在发布包里等于**永远起不来**（包里根本没有 Kotlin）。
///
/// 四种情况：
///  1. **文件不存在**（或存在但是空文件）→ 建父目录 → 建库 → 执行 [`SCHEMA_SQL`] → `user_version = SCHEMA_VERSION`；
///  2. `user_version == 0`（老库 / 手工建的库）→ 执行一遍 DDL（脚本是**收敛式**的：
///     `CREATE TABLE IF NOT EXISTS` + `DROP TRIGGER IF EXISTS` + `CREATE TRIGGER`，重跑安全）→ 写版本号；
///  3. `version >= SCHEMA_VERSION` → **先核对 DDL 里声明的表是不是都在**：都在就什么都不做
///     （**绝不重跑 DDL**：那会重建触发器），缺了就补一遍（见下面那条 ⚠）；
///  4. `version > SCHEMA_VERSION` → **只警告不拦**（拦了用户就没法回退版本），
///     并且**绝不把版本号往小写**（只往下写、不往回落）。
///
/// ⚠ **为什么第 3 类里那道"表都在吗"的检查一个字都不能省**（2026-09-27 真机实测抓到）：
/// `db/schema.sqlite.sql` 是**两端共用**的一份文件，谁往里加一张表都行；而"加了表、
/// 忘了 bump [`SCHEMA_VERSION`]"的表现是**静默的**：
/// 老库停在旧版本号 → `version >= SCHEMA_VERSION` 直接早退 → **那张表永远不存在**，
/// 直到某个端点报 `no such table`（而那时候没人会想到是迁移）。
///
/// 真机现场就是这样：库停在 `user_version=1`，schema 里**有** `workflow_recipes` 的完整 DDL，
/// 而库里**没有这张表**（`character_cards` 有、它没有 —— 差别只在于 DDL 是什么时候加进去的）。
/// 现在这条是**自愈**的：缺哪张补哪张，并且**打一行日志把补了什么说出来**（绝不静默）。
pub fn open_or_create(path: impl AsRef<Path>) -> Result<Connection, String> {
    let path = path.as_ref();
    // **先处理"上次点了恢复"的标记，再开库**（`infra/backup.rs`）。
    // 顺序不能反：活着的连接上换文件，SQLite 会把两个库的页混着读 —— 那种损坏不报错。
    // 它在没有标记时只是一次 `exists()`，所以每条测试 / 每次冷启动都可以放心调。
    crate::infra::backup::apply_pending(path);
    let fresh = !path.exists()
        || std::fs::metadata(path)
            .map(|m| m.len() == 0)
            .unwrap_or(true);
    if fresh {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("建库目录失败（{}）：{e}", parent.display()))?;
        }
    }
    let conn = open(path).map_err(|e| format!("开库失败（{}）：{e}", path.display()))?;
    let version = schema_version(&conn).map_err(|e| format!("读 user_version 失败：{e}"))?;
    let mut missing: Vec<&'static str> = Vec::new();

    // ★★ **先补列，再跑 DDL —— 顺序不能反**（2026-09-30 用真库副本实测抓到）：
    //   DDL 里可能有 `CREATE INDEX ... ON <表>(<新列>)`，而收敛式 DDL **不给已存在的表补列** ——
    //   索引语句排在补列之前，那一刻列还不存在 → **整批 DDL 失败、库直接打不开**
    //   （现场：`e2e-pair-test.ps1` 拿真库副本起后端，报
    //    `打开/建库失败：… no such column: enabled in CREATE INDEX … idx_character_cards_enabled`）。
    //   ⚠ 为什么单测没抓到：它们都用**全新库**（DDL 建表时自带新列），而这只有"老库升级"才会踩到。
    //   `ensure_columns` 对不存在的表是空操作，所以在全新库上这么调也不亏。
    //   （2026-10 角色卡收敛后，DDL 里已经**没有**建在新列上的索引了；顺序照旧保留 ——
    //    哪天再给新列加索引，就是同一条坑。）
    let added = ensure_columns(&conn).map_err(|e| format!("补列失败：{e}"))?;
    if !added.is_empty() {
        eprintln!(
            "[db] 老库缺 {} 列（{}）—— 用 ALTER 补上了；已有数据不动。",
            added.len(),
            added.join("、")
        );
    }
    // 补完列**立刻搬一次数据**：角色卡从 `tags`（JSON 数组）收敛成 `prompt`（整段文本），
    // 不搬就是把老卡读成空卡（列还在、界面却没内容）。
    // ⚠ **只在刚补上这一列的那一次**搬（判据是 `added`）—— 老库的 `tags` 列留在原地，
    //   每次开库都搬会把用户在界面上清空的提示词"复活"，见函数头那条 ⚠。
    if added.iter().any(|c| c == "character_cards.prompt") {
        let migrated =
            backfill_character_card_prompt(&conn).map_err(|e| format!("迁移角色卡失败：{e}"))?;
        if migrated > 0 {
            eprintln!("[db] 角色卡：把 {migrated} 张老卡的 tag 栈拼成了正面提示词（老列不动）。");
        }
    }
    // 2026-10-24：卡片回到单版本。升级到 schemaVersion 11 的那一次，把老库里残留的
    // `tag_prompt` **转成自然语言**灌进 `prompt`（只填空卡；已有正文的一个字节都不动）。
    // ⚠ 只在 `version < 11` 时跑：老列留在原地，每次开库都跑会把用户清空的提示词复活。
    if version < 11 {
        let migrated =
            migrate_card_tags_to_prompt(&conn).map_err(|e| format!("迁移卡片 tag 版失败：{e}"))?;
        if migrated > 0 {
            eprintln!(
                "[db] 卡片：把 {migrated} 张只有 tag 版的卡转成了自然语言（已有正文的卡没动）。"
            );
        }
    }

    if version >= SCHEMA_VERSION && !fresh {
        if version > SCHEMA_VERSION {
            eprintln!(
                "⚠ 库的 schemaVersion={version} 比这个后端认识的（{SCHEMA_VERSION}）**新**：\
                 它可能是更新的版本建的。继续用会有列对不上的风险，建议升级后端。"
            );
        }
        // ★ 自愈（见函数头那条 ⚠）：版本号够新**不等于**结构都对 ——
        //   "表在不在"与"列在不在"是两件事（后者刚在上面那一步补过）。
        missing = missing_tables(&conn).map_err(|e| format!("查缺表失败：{e}"))?;
        if missing.is_empty() {
            return Ok(conn);
        }
        eprintln!(
            "[db] 库里缺 {} 张 DDL 声明过的表（{}）—— 多半是**加了表没 bump SCHEMA_VERSION**。\
             跑一遍收敛式 DDL 补上；已有数据不动。",
            missing.len(),
            missing.join("、")
        );
    }
    // 建表 / 收敛式迁移：整个 DDL 在一个事务里跑（中途失败不留半个 schema）
    conn.execute_batch("BEGIN IMMEDIATE;")
        .map_err(|e| format!("开事务失败：{e}"))?;
    match conn.execute_batch(SCHEMA_SQL) {
        Ok(()) => {
            // ⚠ **版本号取 `max(现有, SCHEMA_VERSION)`**，两个方向都要照顾到：
            //  * **只往下写、绝不往回落**：库比后端新时保留它自己的版本号
            //    （写回 SCHEMA_VERSION 等于把用户的库降级，而那正是"用旧二进制写新库"最危险的事）；
            //  * **但也不能被 DDL 里的那行拖回去**：`db/schema.sqlite.sql` 末尾自己也有一行
            //    `PRAGMA user_version = N`（它是两端共用的文件、Kotlin 侧也读它），
            //    而"库够新、只是缺张表"那条自愈路径里我们**刚跑过一遍 DDL** ——
            //    如果照搬"DDL 里的值"，版本号就会从 2 **退回 1**（2026-09-27 实测踩到：
            //    自愈成功、表补上了，`user_version` 却掉回 1）。
            //
            // ⚠ **判据要比"DDL 跑完之后库里的实际值"，不能比"跑 DDL 之前读到的 version"**
            //   （2026-09-30 补：原来写的是 `if target != version`，那只在"版本比后端旧"时成立 ——
            //   库是 99、只是缺一张表时，DDL 把版本写成 5，而 `target(99) != version(99)` 为假，
            //   于是**该写的那次没写**，库被静默降级到 5。用例
            //   `healing_a_missing_table_never_downgrades_a_newer_version` 钉住这条）。
            let target = version.max(SCHEMA_VERSION);
            let after_ddl =
                schema_version(&conn).map_err(|e| format!("读 user_version 失败：{e}"))?;
            if after_ddl != target {
                conn.execute_batch(&format!("PRAGMA user_version = {target};"))
                    .map_err(|e| format!("写 user_version 失败：{e}"))?;
            }
            conn.execute_batch("COMMIT;")
                .map_err(|e| format!("提交失败：{e}"))?;
            if fresh {
                eprintln!(
                    "[db] 已建库并建表：{}（schemaVersion={SCHEMA_VERSION}）",
                    path.display()
                );
            } else if missing.is_empty() {
                eprintln!(
                    "[db] 已把老库收敛到 schemaVersion={SCHEMA_VERSION}：{}",
                    path.display()
                );
            } else {
                eprintln!(
                    "[db] 已补上 {} 张缺的表（{}）：{}",
                    missing.len(),
                    missing.join("、"),
                    path.display()
                );
            }
            Ok(conn)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(format!("执行 db/schema.sqlite.sql 失败：{e}"))
        }
    }
}

/// 建表用的 DDL 文本（给用例与"手工建库"用）。
pub fn schema_sql() -> &'static str {
    SCHEMA_SQL
}

/// DDL 里**声明的**表名（`CREATE TABLE IF NOT EXISTS <name>`）。
///
/// 它是 [`open_or_create`] 那条自愈检查的判据，也是"加表忘了 bump 版本"这条静默故障的
/// 唯一解药。⚠ 解析的是**我们自己的** DDL 文本，所以判据只能是"这一种写法"——
/// 哪天有人用别的写法建表（换行 / 加引号 / `CREATE VIRTUAL TABLE`），
/// `every_declared_table_is_actually_created` 会当场变红（那条用例盯的正是这个）。
pub fn declared_tables() -> Vec<&'static str> {
    const MARK: &str = "CREATE TABLE IF NOT EXISTS ";
    let mut out: Vec<&'static str> = Vec::new();
    let mut rest = SCHEMA_SQL;
    while let Some(at) = rest.find(MARK) {
        let tail = &rest[at + MARK.len()..];
        let name = tail
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or("");
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
        rest = tail;
    }
    out
}

/// 库里**缺**哪些 DDL 声明过的表（空 = 一切正常）。
fn missing_tables(conn: &Connection) -> rusqlite::Result<Vec<&'static str>> {
    let mut out: Vec<&'static str> = Vec::new();
    for name in declared_tables() {
        let found: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            rusqlite::params![name],
            |row| row.get(0),
        )?;
        if found == 0 {
            out.push(name);
        }
    }
    Ok(out)
}

pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(PRAGMAS)?;
    assert_ready(&conn)?;
    Ok(conn)
}

/// 只读连接（`/api/*` 的读端点用它；`mode=ro` 时**不会**因为并发写而拿不到库）。
pub fn open_readonly(path: impl AsRef<Path>) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // 只读连接同样要上 `busy_timeout`、`foreign_keys` 与 `temp_store`：R1 没有例外，
    // 而 temp_store 也是连接级的（只读连接照样会做排序 → 照样可能要临时文件）
    conn.execute_batch(
        "PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON; PRAGMA temp_store = MEMORY;",
    )?;
    assert_ready(&conn)?;
    Ok(conn)
}

/// 内存库（单测用；DDL 由调用方从 `db/schema.sqlite.sql` 灌进去）。
pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    // 内存库没有 WAL（返回 memory），其余 PRAGMA 照上
    conn.execute_batch(
        "PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON; \
         PRAGMA temp_store = MEMORY;",
    )?;
    assert_ready(&conn)?;
    Ok(conn)
}

/// **把该为真的东西读回来核对**（R1 / R14）—— 不对就当场抛，绝不允许"带着坏掉的库继续跑"。
pub fn assert_ready(conn: &Connection) -> rusqlite::Result<()> {
    let fk: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if fk != 1 {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "R1 未生效：PRAGMA foreign_keys={fk}（应为 1）—— 外键会静默失效"
        )));
    }
    // 与 R1 同一种纪律：**读回来核对**，别"写了就算"
    assert_temp_store_is_memory(conn)?;
    Ok(())
}

/// 引擎信息（`/api/health` 用；也是 R14 的自检口径）。
pub fn engine_info(conn: &Connection) -> rusqlite::Result<serde_json::Value> {
    let version: String = conn.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
    // FTS5 探针：系统 SQLite（Android 13 的 3.32a）没有它；
    // 桌面 bundled 版必须有 —— 缺了就是选型出了问题，要当场看得见
    let fts5 = conn
        .execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS temp.fts_probe USING fts5(x); DROP TABLE IF EXISTS temp.fts_probe;")
        .is_ok();
    Ok(serde_json::json!({
        "sqliteVersion": version,
        "fts5": fts5,
    }))
}

/// schema 版本（= `/api/sync/manifest` 与 run-kit 说的**同一个数**）。
pub fn schema_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
}

/// 写事务：**`BEGIN IMMEDIATE`**（R2）+ 出错自动回滚。
///
/// 错误类型做成泛型（`E: From<rusqlite::Error>`）：仓储层有自己的错误分类
/// （[`crate::library::repo::WriteError`]：400 / 404 / 500），但**事务纪律只有这一处实现** ——
/// 别为了换个错误类型就在别处再拼一遍 `BEGIN IMMEDIATE`。
pub fn tx_immediate<T, E>(
    conn: &mut Connection,
    f: impl FnOnce(&Transaction<'_>) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<rusqlite::Error>,
{
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}

#[cfg(test)]
mod tests;
