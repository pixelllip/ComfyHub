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

/// 一张卡的写入形状：`input("名字", "正面提示词")`。
fn input(name: &str, prompt: &str) -> CharacterCardInput {
    CharacterCardInput {
        name: name.to_string(),
        prompt: Some(prompt.to_string()),
        ..Default::default()
    }
}

#[test]
fn 建卡读回形状() {
    let conn = conn();
    let mut card = input("Shinku", "shinku_(nte), pink streaks, red eyes");
    card.negative = Some("bad hands".into());
    let id = create(&conn, &card).unwrap();

    let got = get(&conn, id).unwrap().unwrap();
    assert_eq!(got.name, "Shinku");
    assert_eq!(got.prompt.as_deref(), Some("shinku_(nte), pink streaks, red eyes"));
    assert_eq!(got.negative.as_deref(), Some("bad hands"));
    assert!(got.created_at.is_some() && got.updated_at.is_some(), "时间列要有");
    // 可空字段不出现（Kotlin `explicitNulls = false`）
    let json = serde_json::to_value(&got).unwrap();
    assert!(json.as_object().unwrap().contains_key("prompt"));
    // 卡上只有这三个字段（2026-10 收敛：多了任何一个都是回归）
    let mut keys: Vec<&String> = json.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["createdAt", "id", "name", "negative", "prompt", "updatedAt"],
        "角色卡只许有名称 / 正面提示词 / 负面提示词（+ id 与时间戳）"
    );
}

#[test]
fn 空提示词落_null_而不是空串() {
    let conn = conn();
    let mut card = input("Shinku", "   ");
    card.negative = Some(String::new());
    let id = create(&conn, &card).unwrap();
    let got = get(&conn, id).unwrap().unwrap();
    assert!(got.prompt.is_none() && got.negative.is_none(), "空白一律落 NULL");
    let json = card_json(&got);
    assert!(json.get("prompt").is_none() && json.get("negative").is_none(), "整键省略：{json}");
}

#[test]
fn 同名是同一张卡_合并保存不抹掉上一轮() {
    let conn = conn();
    let mut first = input("Iroi", "iroi_(nte), green hair, sheep horns");
    first.negative = Some("bad hands".into());
    let (id1, created1) = save(&conn, first).unwrap();
    assert!(created1);

    // 第二轮模型只说了一件事：负面词（大小写不同的同名照样是同一张卡）
    let second = CharacterCardInput {
        name: "iroi".into(),
        negative: Some("extra fingers".into()),
        ..Default::default()
    };
    let (id2, created2) = save(&conn, second).unwrap();
    assert!(!created2, "同名必须走合并");
    assert_eq!(id1, id2);

    let got = get(&conn, id1).unwrap().unwrap();
    assert_eq!(got.negative.as_deref(), Some("extra fingers"), "给了值就覆盖");
    assert_eq!(
        got.prompt.as_deref(),
        Some("iroi_(nte), green hair, sheep horns"),
        "这次没提的字段不许被抹掉"
    );
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM character_cards", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "同名不许攒出第二行");
}

#[test]
fn 提示词是整段替换不是追加() {
    let conn = conn();
    let (id, _) = save(&conn, input("Shinku", "shinku_(nte)")).unwrap();
    save(&conn, input("Shinku", "shinku_(nte), red eyes")).unwrap();
    let got = get(&conn, id).unwrap().unwrap();
    assert_eq!(
        got.prompt.as_deref(),
        Some("shinku_(nte), red eyes"),
        "追加会把两段风格不同的提示词粘成一条，读起来像一条、其实是两条"
    );
}

#[test]
fn 全量替换会清掉没给的字段() {
    let conn = conn();
    let mut card = input("Shinku", "原来有正面提示词");
    card.negative = Some("原来有负面词".into());
    let id = create(&conn, &card).unwrap();
    // 界面弹窗的 PUT = 全量替换：空就是清空
    let replaced = CharacterCardInput { name: "Shinku".into(), ..Default::default() };
    assert!(update(&conn, id, &replaced).unwrap());
    let got = get(&conn, id).unwrap().unwrap();
    assert!(got.prompt.is_none(), "PUT 是全量替换（弹窗里所有字段都在）");
    assert!(got.negative.is_none());
}

#[test]
fn 字段超长报错而不是截断() {
    let conn = conn();
    let mut card = input("Shinku", "ok");
    card.negative = Some("x".repeat(MAX_PROMPT_CHARS + 1));
    let err = create(&conn, &card).unwrap_err();
    let message = format!("{err:?}");
    assert!(message.contains("负面提示词 超出上限"), "{message}");
    assert!(message.contains("不要指望被悄悄截断"), "{message}");
    // 名字为空 / 太长同样是 400 那一档
    assert!(create(&conn, &CharacterCardInput::default()).is_err());
    let long_name = "名".repeat(MAX_NAME_CHARS + 1);
    assert!(create(&conn, &input(&long_name, "ok")).is_err());
}

#[test]
fn 搜索命中名字与提示词() {
    let conn = conn();
    let mut card = input("Shinku", "shinku_(nte), pink streaks");
    card.negative = Some("neverness_to_everness".into());
    create(&conn, &card).unwrap();
    create(&conn, &input("Iroi", "iroi_(nte), green hair")).unwrap();

    assert_eq!(list(&conn, None, MAX_SEARCH).unwrap().len(), 2, "q 为空 = 列全部");
    assert_eq!(list(&conn, Some("shin"), MAX_SEARCH).unwrap().len(), 1, "名字要命中");
    assert_eq!(list(&conn, Some("pink streaks"), MAX_SEARCH).unwrap().len(), 1, "正面提示词要命中");
    assert_eq!(list(&conn, Some("everness"), MAX_SEARCH).unwrap().len(), 1, "负面提示词要命中");
    assert!(list(&conn, Some("不存在的东西"), MAX_SEARCH).unwrap().is_empty());
}

#[test]
fn 工具_搜索查不到时不许编并且要说下一步() {
    let state = test_state();
    let err = tool_search(&state, &serde_json::json!({"query": "shinku"})).unwrap_err();
    assert_eq!(err.code, "NOT_FOUND");
    assert!(err.message.contains("shinku"), "{}", err.message);
    assert!(err.message.contains("danbooru_lookup"), "{}", err.message);
    assert!(err.message.contains("save_character_card"), "{}", err.message);
    assert!(err.message.contains("不要"), "{}", err.message);
}

#[test]
fn 工具_保存是合并保存_没给提示词要如实提醒() {
    let state = test_state();
    // 第一轮只说名字：存得下，但要提醒"这张卡还是空的"（不许拿印象糊上去）
    let first = tool_save(&state, &serde_json::json!({ "name": "Shinku" })).unwrap();
    assert!(first.content.contains("已保存角色卡 #1「Shinku」（新建）"), "{}", first.content);
    assert!(first.content.contains("没有正面提示词"), "{}", first.content);
    assert!(
        !first.payload.as_object().unwrap().contains_key("prompt"),
        "没给就不许编一个：{}",
        first.payload
    );

    // 第二轮带着提示词回来 —— 合并保存，卡上就有内容了
    let second = tool_save(
        &state,
        &serde_json::json!({
            "name": "shinku",
            "prompt": "shinku_(nte), pink streaks",
            "negative": "bad hands"
        }),
    )
    .unwrap();
    assert!(second.content.contains("（合并更新）"), "{}", second.content);
    assert_eq!(second.payload["id"], 1);
    assert_eq!(second.payload["prompt"], "shinku_(nte), pink streaks");
    assert_eq!(second.payload["negative"], "bad hands");
    assert!(!second.content.contains("没有正面提示词"), "{}", second.content);

    // 第三轮只说负面词：正面提示词必须原样留着
    let third = tool_save(&state, &serde_json::json!({ "name": "shinku", "negative": "lowres" })).unwrap();
    assert_eq!(
        third.payload["prompt"],
        "shinku_(nte), pink streaks",
        "合并保存不许把上一轮写的正面提示词抹掉：{}",
        third.payload
    );
    assert_eq!(third.payload["negative"], "lowres");
    let count: i64 = state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM character_cards", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(count, 1, "同名不许攒出第二行");
}

#[test]
fn 工具_缺名字与超长都是参数错误() {
    let state = test_state();
    let err = tool_save(&state, &serde_json::json!({})).unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(err.message.contains("name"), "{}", err.message);

    let long = "x".repeat(MAX_PROMPT_CHARS + 1);
    let err = tool_save(&state, &serde_json::json!({ "name": "x", "negative": long })).unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(err.message.contains("负面提示词 超出上限"), "{}", err.message);
}

fn test_state() -> AppState {
    // 内存库要自己灌真 DDL（`AppState::new` 只包连接，不建表）
    let conn = conn();
    AppState::new(conn, ":memory:", crate::infra::testpaths::scratch("media"))
}

#[test]
fn 删除与不存在的行为() {
    let conn = conn();
    let id = create(&conn, &input("Shinku", "ok")).unwrap();
    assert!(delete(&conn, id).unwrap());
    assert!(!delete(&conn, id).unwrap(), "删第二次要如实说没删到");
    assert!(get(&conn, id).unwrap().is_none());
    assert!(update(&conn, id, &input("Shinku", "ok")).unwrap() == false);
}

/// `render_card` 是给模型看的那份正文：**空提示词要显式说"（空）"** ——
/// 少写一行，模型就会以为卡里有内容而照着"印象"往下写。
#[test]
fn 正文形状把空提示词显式画出来() {
    let conn = conn();
    let id = create(&conn, &input("Shinku", "shinku_(nte)")).unwrap();
    let card = get(&conn, id).unwrap().unwrap();
    let text = render_card(&card);
    assert!(text.contains("#1「Shinku」"), "{text}");
    assert!(text.contains("正面提示词: shinku_(nte)"), "{text}");
    assert!(!text.contains("负面提示词"), "没填的字段不占一行：{text}");

    let empty = CharacterCardDto {
        id: 2,
        name: "空的".into(),
        prompt: None,
        negative: None,
        created_at: None,
        updated_at: None,
    };
    assert!(render_card(&empty).contains("正面提示词:（空）"), "{}", render_card(&empty));
}
