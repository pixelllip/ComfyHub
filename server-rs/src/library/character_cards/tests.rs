//! `character_cards` 的单元测试（主体代码在 [`character_cards`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn conn() -> Connection {
    let conn = crate::infra::db::open_memory().unwrap();
    // 内存库不带 DDL：真 schema 从 `db/schema.sqlite.sql` 灌（与 `tests/api_test.rs` 同一条路；
    // 用真 DDL 而不是测试里另写一份建表语句 —— 否则 `normalized` 唯一索引这类
    // "承重的约束"在用例里根本不存在，测出来的绿是假的）。
    let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("db")
        .join("schema.sqlite.sql");
    let schema = std::fs::read_to_string(&schema_path)
        .unwrap_or_else(|e| panic!("读不到 {}：{e}", schema_path.display()));
    conn.execute_batch(&schema).expect("建 schema");
    crate::infra::db::assert_ready(&conn).unwrap();
    conn
}

fn input(name: &str, tags: &[&str]) -> CharacterCardInput {
    CharacterCardInput {
        name: name.to_string(),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

#[test]
fn 建卡读回形状与_kotlin_一致() {
    let conn = conn();
    let mut card = input("Shinku", &["shinku_(nte)", "pink streaks"]);
    card.character_tag = Some("shinku_(nte)".into());
    card.lora_name = Some("shinku-anima".into());
    card.lora_weight = Some(0.85);
    card.source = Some("danbooru".into());
    let id = create(&conn, &card).unwrap();

    let got = get(&conn, id).unwrap().unwrap();
    assert_eq!(got.name, "Shinku");
    assert_eq!(got.tags, vec!["shinku_(nte)".to_string(), "pink streaks".to_string()]);
    assert_eq!(got.lora_weight, Some(0.85));
    assert_eq!(got.character_tag.as_deref(), Some("shinku_(nte)"));
    assert!(got.created_at.is_some() && got.updated_at.is_some(), "时间列要有");
    // 可空字段不出现（Kotlin `explicitNulls = false`）
    let json = serde_json::to_value(&got).unwrap();
    assert!(!json.as_object().unwrap().contains_key("negative"));
    assert!(!json.as_object().unwrap().contains_key("seriesTag"));
    assert!(json.as_object().unwrap().contains_key("aliases"), "空数组照样给键");
}

#[test]
fn 同名是同一张卡_合并保存不抹掉上一轮() {
    let conn = conn();
    let mut first = input("Iroi", &["green hair", "sheep horns"]);
    first.source = Some("civitai 触发词".into());
    first.aliases = vec!["iroi_(nte)".into()];
    let (id1, created1) = save(&conn, first).unwrap();
    assert!(created1);

    // 第二轮模型只说了一件事：LoRA 名
    let mut second = CharacterCardInput { name: "iroi".into(), ..Default::default() };
    second.lora_name = Some("iroi-anima".into());
    second.tags = vec!["white dress".into()];
    let (id2, created2) = save(&conn, second).unwrap();
    assert!(!created2, "同名必须走合并");
    assert_eq!(id1, id2);

    let got = get(&conn, id1).unwrap().unwrap();
    assert_eq!(got.lora_name.as_deref(), Some("iroi-anima"));
    assert_eq!(
        got.tags,
        vec!["green hair".to_string(), "sheep horns".to_string(), "white dress".to_string()],
        "tags 是追加去重，不许覆盖"
    );
    assert_eq!(got.source.as_deref(), Some("civitai 触发词"), "来源不能被空值抹掉");
    assert_eq!(got.aliases, vec!["iroi_(nte)".to_string()]);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM character_cards", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "同名不许攒出第二行");
}

#[test]
fn 全量替换会清掉没给的字段() {
    let conn = conn();
    let mut card = input("Shinku", &["a"]);
    card.notes = Some("原来有备注".into());
    let id = create(&conn, &card).unwrap();
    let replaced = input("Shinku", &["b"]);
    assert!(update(&conn, id, &replaced).unwrap());
    let got = get(&conn, id).unwrap().unwrap();
    assert_eq!(got.tags, vec!["b".to_string()]);
    assert!(got.notes.is_none(), "PUT 是全量替换（界面弹窗里所有字段都在）");
}

#[test]
fn 字段超长报错而不是截断() {
    let conn = conn();
    let mut card = input("Shinku", &[]);
    card.notes = Some("x".repeat(MAX_NOTES_CHARS + 1));
    let err = create(&conn, &card).unwrap_err();
    let message = format!("{err:?}");
    assert!(message.contains("备注 超出上限"), "{message}");
    assert!(message.contains("不要指望被悄悄截断"), "{message}");
    // 名字为空 / 太长同样是 400 那一档
    assert!(create(&conn, &CharacterCardInput::default()).is_err());
    let long_name = "名".repeat(MAX_NAME_CHARS + 1);
    assert!(create(&conn, &input(&long_name, &[])).is_err());
}

#[test]
fn 参考图不存在时报人话而不是外键_500() {
    let conn = conn();
    let mut card = input("Shinku", &[]);
    card.reference_media_id = Some(999_999);
    let err = create(&conn, &card).unwrap_err();
    let message = format!("{err:?}");
    assert!(message.contains("产物 999999") && message.contains("不存在"), "{message}");
}

#[test]
fn tag_清洗与拆分的三条纪律() {
    // 三种分隔符都认，空格分隔**不认**
    assert_eq!(
        split_tags("red eyes, blue hair，white dress\nsheep horns;green hair；pink"),
        vec!["red eyes", "blue hair", "white dress", "sheep horns", "green hair", "pink"]
    );
    // 去重保序
    assert_eq!(split_tags("a, b, a"), vec!["a", "b"]);
    // 列表符号与序号
    assert_eq!(normalize_tag("- pink streaks"), "pink streaks");
    assert_eq!(normalize_tag("2. sheep horns"), "sheep horns");
    assert_eq!(normalize_tag("3、green hair"), "green hair");
    assert_eq!(normalize_tag("  很多    空格 "), "很多 空格");
    // 上限：100 个 tag、96 字符一个
    let many: Vec<String> = (0..150).map(|i| format!("tag{i}")).collect();
    assert_eq!(clean_tags(&many).len(), MAX_TAGS);
    assert_eq!(normalize_tag(&"x".repeat(200)).chars().count(), MAX_TAG_CHARS);
}

#[test]
fn 搜索命中名字别名与_tag_栈() {
    let conn = conn();
    let mut card = input("Shinku", &["pink streaks", "purple horns"]);
    card.aliases = vec!["真红".into()];
    card.notes = Some("neverness_to_everness".into());
    create(&conn, &card).unwrap();
    create(&conn, &input("Iroi", &["green hair"])).unwrap();

    assert_eq!(list(&conn, None, MAX_SEARCH).unwrap().len(), 2, "q 为空 = 列全部");
    assert_eq!(list(&conn, Some("shin"), MAX_SEARCH).unwrap().len(), 1);
    assert_eq!(list(&conn, Some("真红"), MAX_SEARCH).unwrap().len(), 1, "别名要命中");
    assert_eq!(list(&conn, Some("purple horns"), MAX_SEARCH).unwrap().len(), 1, "tag 栈要命中");
    assert_eq!(list(&conn, Some("everness"), MAX_SEARCH).unwrap().len(), 1, "备注要命中");
    assert!(list(&conn, Some("不存在的东西"), MAX_SEARCH).unwrap().is_empty());
}

#[test]
fn 工具_搜索查不到时不许编并且要说下一步() {
    let state = test_state();
    let err = tool_search(&state, &serde_json::json!({"query": "shinku"})).unwrap_err();
    assert_eq!(err.code, "NOT_FOUND");
    assert!(err.message.contains("shinku"), "{}", err.message);
    assert!(err.message.contains("danbooru_lookup"), "{}", err.message);
    assert!(err.message.contains("不要**因为") || err.message.contains("不要"), "{}", err.message);
}

#[test]
fn 工具_保存是合并保存且没查过_danbooru_就如实留空() {
    let state = test_state();
    let session = crate::tools::tools::ToolSession::default();
    let first = tool_save(
        &state,
        &serde_json::json!({
            "name": "Shinku",
            "characterTag": "shinku_(nte)",
            "tags": "pink streaks, purple horns",
            "source": "danbooru"
        }),
        &session,
    )
    .unwrap();
    assert!(first.content.contains("已保存角色卡 #1「Shinku」（新建）"), "{}", first.content);
    assert!(first.content.contains("角色 tag: shinku_(nte)"), "{}", first.content);
    assert!(first.content.contains("tag 栈(2)"), "{}", first.content);
    // 这一轮**没查过** danbooru：核对时间必须如实留空 + 明确提醒
    assert!(first.content.contains("**没有** danbooru 核对记录"), "{}", first.content);
    assert!(
        !first.payload.as_object().unwrap().contains_key("tagsVerifiedAt"),
        "没查过就不许出现核对时间键：{}",
        first.payload
    );

    // 第二轮只说 LoRA —— 合并保存，tag 栈与来源都不许被抹掉
    let second = tool_save(
        &state,
        &serde_json::json!({ "name": "shinku", "loraName": "shinku-anima", "loraWeight": 0.85 }),
        &session,
    )
    .unwrap();
    assert!(second.content.contains("（合并更新）"), "{}", second.content);
    assert_eq!(second.payload["id"], 1);
    assert_eq!(
        second.payload["tags"],
        serde_json::json!(["pink streaks", "purple horns"])
    );
    assert_eq!(second.payload["source"], "danbooru");
    assert_eq!(second.payload["loraName"], "shinku-anima");
    let count: i64 = state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM character_cards", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(count, 1, "同名不许攒出第二行");
}

/// **核对时间是事实、不是自述**：只有这一轮 `danbooru_lookup` 真的查过那个 tag
/// （`ToolSession.danbooru_verified` 里有归一之后的键）才写得进去。
///
/// 两条方向都要盯：查过 → 必须写；没查过（哪怕模型把 `source` 写成 `danbooru`）→ 必须留空。
#[test]
fn 工具_查过_danbooru_才写得进核对时间() {
    let state = test_state();
    let mut session = crate::tools::tools::ToolSession::default();
    // 查的是 zh 写法 → 归一之后同一个键（`Shinku (NTE)` 与 `shinku_(nte)` 是一回事）
    session.mark_danbooru_verified("Shinku (NTE)");
    let saved = tool_save(
        &state,
        &serde_json::json!({ "name": "shinku", "characterTag": "shinku_(nte)" }),
        &session,
    )
    .unwrap();
    assert!(saved.content.contains("danbooru 核对时间已记上"), "{}", saved.content);
    let verified_at = saved.payload["tagsVerifiedAt"].as_str().unwrap_or("");
    assert!(verified_at.len() >= 20 && verified_at.ends_with('Z'), "核对时间要是 ISO：{verified_at}");

    // 另一个 tag 没查过 → 再存一次不许有核对时间（合并保存也不会把它带出来）
    let other = tool_save(
        &state,
        &serde_json::json!({ "name": "iroi", "characterTag": "iroi_(nte)", "source": "danbooru" }),
        &session,
    )
    .unwrap();
    assert!(other.content.contains("**没有** danbooru 核对记录"), "{}", other.content);
    assert!(!other.payload.as_object().unwrap().contains_key("tagsVerifiedAt"), "{}", other.payload);
}

#[test]
fn 工具_缺名字与超长都是参数错误() {
    let state = test_state();
    let session = crate::tools::tools::ToolSession::default();
    let err = tool_save(&state, &serde_json::json!({}), &session).unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(err.message.contains("name"), "{}", err.message);

    let long = "x".repeat(MAX_NOTES_CHARS + 1);
    let err = tool_save(&state, &serde_json::json!({ "name": "x", "notes": long }), &session).unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(err.message.contains("备注 超出上限"), "{}", err.message);
}

fn test_state() -> AppState {
    // 内存库要自己灌真 DDL（`AppState::new` 只包连接，不建表）
    let conn = conn();
    AppState::new(conn, ":memory:", crate::infra::testpaths::scratch("media"))
}

#[test]
fn 删除与不存在的行为() {
    let conn = conn();
    let id = create(&conn, &input("Shinku", &[])).unwrap();
    assert!(delete(&conn, id).unwrap());
    assert!(!delete(&conn, id).unwrap(), "删第二次要如实说没删到");
    assert!(get(&conn, id).unwrap().is_none());
    assert!(update(&conn, id, &input("Shinku", &[])).unwrap() == false);
}
