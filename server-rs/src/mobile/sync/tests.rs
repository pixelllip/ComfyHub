//! `sync` 的单元测试（主体代码在 [`sync`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

/// **没有任何一张"带凭据/令牌"的表能出现在 `synced: true` 里。**
///
/// 这条用例盯着白名单本身：谁把 `app_settings` 加进 [SYNCED]（哪怕理由是
/// "设置也该同步到手机"），这里立刻红。
#[test]
fn credentials_and_settings_are_never_synced() {
    for dangerous in ["app_settings", "ai_providers", "ai_models", "ai_run_events"] {
        assert!(!SYNCED.contains(&dangerous), "{dangerous} 绝不能被同步");
        let entry = NOT_SYNCED.iter().find(|(n, _)| *n == dangerous);
        assert!(entry.is_some(), "{dangerous} 必须显式列出并给出理由（不能只是'没提到'）");
        assert!(!entry.unwrap().1.trim().is_empty(), "{dangerous} 的理由不能是空的");
    }
}

/// 同步范围与总纲 §3.3 逐字一致（少一张 = 手机永远收不到那种数据）。
#[test]
fn synced_scope_matches_the_master_plan() {
    let expected = [
        "prompts",
        "tags",
        "prompt_tags",
        "media_assets",
        "media_tags",
        // 2026-10-11：产物集合（同源微调归组）。跟着 media_assets 走 ——
        // 少了它，手机上的 `collection_id` 会指向一个它那边不存在的集合。
        "media_collections",
        "ai_conversations",
        "ai_messages",
        "ai_message_parts",
        "ai_attachments",
        "capture_runs",
        // 2026-09-27 §7 待定项 1 = 进（手机显示工具过程）
        "ai_runs",
        "ai_tool_calls",
    ];
    assert_eq!(SYNCED, &expected, "改同步范围要同时改总纲 §3.3 与契约");
    // 两张表不能同时出现在两个清单里
    for name in SYNCED {
        assert!(
            !NOT_SYNCED.iter().any(|(n, _)| n == name),
            "{name} 同时出现在同步与不同步清单里",
        );
    }
}

/// 每张同步表都能被 `changes` 取到（`pk_kind` 漏一张 = 那条账本被**静默跳过**）。
///
/// `changes_on` 里的 `let Some(kind) = pk_kind(&row.tbl) else { continue }` 就是这个坑：
/// 表进了白名单、`pk_kind` 忘了加，表现是"手机收不到这张表的任何变更"，两端都不报错。
#[test]
fn every_synced_table_has_a_pk_kind() {
    for name in SYNCED {
        assert!(pk_kind(name).is_some(), "{name} 在 SYNCED 里，pk_kind 却不认识它");
    }
}

/// 上行 kind 与下行表名**互为同一批表**（只有列表这一侧加了、上行那侧忘了，
/// 表现是手机推的 Run 全被 `UNKNOWN_KIND` 永久拒掉 —— 而且是"永久"，不再重试）。
#[test]
fn uplink_kinds_cover_the_phone_writable_tables() {
    assert_eq!(table_of_kind("ai_run"), Some("ai_runs"));
    assert_eq!(table_of_kind("ai_tool_call"), Some("ai_tool_calls"));
    // 手机仍然**不许**创建桌面在提交时产出的那几张（[PHONE_FORBIDDEN]）
    assert!(PHONE_FORBIDDEN.contains(&"prompts"));
    assert!(PHONE_FORBIDDEN.contains(&"capture_runs"));
    // 两个方向认识的是同一批表名（写错一个字母不会报错，只会"永远收不到"）
    for kind in ["ai_conversation", "ai_message", "ai_message_part", "ai_attachment", "ai_run"] {
        let table = table_of_kind(kind).expect("kind 必须认识");
        assert!(SYNCED.contains(&table), "{kind} → {table}，但 {table} 不在同步范围里");
    }
    // ★ `submit_job` **不是表**（契约 §4.1）：它的分支在 `apply_one` 最前面。
    //   真有谁"顺手"把它加进这张映射表，就会走到按列名 upsert 那条路上 ——
    //   而 payload 的键（queueId / conversationId / …）跟 `submit_jobs` 的列不是一回事，
    //   表现是一律 `SCHEMA_MISMATCH`（永久拒），手机上永远排不上队。
    assert_eq!(table_of_kind("submit_job"), None, "submit_job 是一条指令，不是一张表");
}

#[test]
fn kit_version_key_matches_the_kotlin_side() {
    // 键名写错不会报错，只会让 kitVersion 永远为 null（手机以为规则工件过期）
    assert_eq!(KEY_KIT_VERSION, "ai.runkit.version");
}

/// 真源是仓库里那份 DDL（`db/schema.sqlite.sql`）—— 与 `events.rs` / `character_cards.rs`
/// 读它的方式一致：**二进制里那份是编译期嵌进去的副本，这里要读的是文件本身**。
fn schema_text() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("db")
        .join("schema.sqlite.sql");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()))
}

/// 取含 `needle` 的那条语句（DDL 里每条触发器正好两行：`CREATE …` + `BEGIN … END;`）。
fn statement_with(sql: &str, needle: &str) -> String {
    sql.lines()
        .skip_while(|line| !line.contains(needle))
        .take(2)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `pk_kind` 认为触发器里应该长什么样的 pk 表达式（两边漂了就是"取不到那一行"）。
///
/// `prefix` 是 `NEW`（INSERT / UPDATE）或 `OLD`（DELETE）—— 删除触发器的 pk 取的是**旧行**，
/// 这正是"删了以后按主键回查"能对上号的原因。
fn expected_pk_expr(kind: PkKind, prefix: &str) -> String {
    match kind {
        PkKind::Int => format!("CAST({prefix}.id AS TEXT)"),
        PkKind::Text => format!("{prefix}.id"),
        PkKind::Composite { sep, parts } => parts
            .iter()
            .enumerate()
            .map(|(index, part)| {
                let piece = match part {
                    PkPart::Int(col) => format!("CAST({prefix}.{col} AS TEXT)"),
                    PkPart::Text(col) => format!("{prefix}.{col}"),
                };
                if index == 0 {
                    piece
                } else {
                    format!("'{sep}'||{piece}")
                }
            })
            .collect::<Vec<_>>()
            .join("||"),
    }
}

/// **白名单 ↔ 触发器逐字对齐**。
///
/// ⚠ 这条以前**只写在 [`pk_kind`] 的注释里、测试并不存在**（2026-09-27 补上）：那段注释
/// 拿它当防线，而真加一张表忘了加触发器时，没有任何东西会红 —— 而表现是"手机永远收不到
/// 这张表的数据"，两端都不报错。所以这里做三件事：
///   ① 同步表必须有 `sync_ai` / `sync_au` / `sync_ad` 三个触发器；
///   ② 触发器的 pk 表达式必须与 [`pk_kind`] 算出来的**逐字相同**（含 `CAST` 与分隔符）；
///   ③ 不同步的表**绝不许**有 `sync_*` 触发器（`touch` 触发器不算 —— 当初就误判过一次）。
#[test]
fn pk_shapes_match_the_schema_triggers() {
    let sql = schema_text();
    for name in SYNCED {
        let kind = pk_kind(name).unwrap_or_else(|| panic!("{name} 在 SYNCED 里却没有 pk_kind"));
        // ⚠ **复合主键的表没有 `au` 触发器**（`prompt_tags` / `media_tags`）：主键就是全部列，
        //   "改一行"在语义上等于"删+插"，所以 DDL 里根本没写 UPDATE 那一条 ——
        //   这不是漏了，别照着"三件套"去补（补了也只是永远不触发的死代码）。
        let suffixes: &[(&str, &str, &str)] = match kind {
            PkKind::Composite { .. } => &[("ai", "upsert", "NEW"), ("ad", "delete", "OLD")],
            _ => &[("ai", "upsert", "NEW"), ("au", "upsert", "NEW"), ("ad", "delete", "OLD")],
        };
        for (suffix, op, prefix) in suffixes {
            let expr = expected_pk_expr(kind, prefix);
            let needle = format!("trg_{name}_sync_{suffix}");
            assert!(
                sql.contains(&needle),
                "DDL 里没有 {needle}（表在白名单里却没触发器 = 手机永远收不到 {name}）",
            );
            let stmt = statement_with(&sql, &needle);
            assert!(
                stmt.contains(&expr),
                "{needle} 的 pk 表达式与 pk_kind 不一致：\n期望包含 {expr}\n实际 {stmt}",
            );
            assert!(stmt.contains(&format!("'{op}'")), "{needle} 的 op 应当是 {op}：{stmt}");
        }
    }
    for (name, _) in NOT_SYNCED {
        assert!(
            !sql.contains(&format!("trg_{name}_sync_")),
            "{name} 在「不同步」清单里，DDL 里却挂了账本触发器 —— \
             要么删触发器，要么把它挪进 SYNCED（并同时改总纲 §3.3）",
        );
    }
}
