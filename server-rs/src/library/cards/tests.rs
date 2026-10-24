//! `cards` 的单元测试：角色卡 / 画风卡共用这一份仓储。
//!
//! 重点盯四条（都是会**静默**出错的）：
//!  ① 正面提示词只有**一版**（2026-10-24 收敛：自然语言），合并保存时没给的字段不许被抹掉；
//!  ② 角色卡与画风卡同名时必须是**两条独立记录**；
//!  ③ 示例产物必须校验存在性、保持加入顺序、上限报错而不是截断；
//!  ④ 工具正文要**明确说出正面提示词还空着** —— 少了这句，模型就会以为卡已经写完整了。

use super::*;

fn conn() -> Connection {
    let conn = crate::infra::db::open_memory().unwrap();
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

fn input(name: &str) -> CardInput {
    CardInput {
        name: name.into(),
        prompt: Some("A quiet girl with red eyes and silver hair.".into()),
        negative: Some("blurry, low quality".into()),
    }
}

fn media(conn: &Connection, name: &str) -> i64 {
    conn.execute(
        "INSERT INTO media_assets (kind, title, original_name, stored_name) \
         VALUES ('IMAGE', ?1, ?1, ?1)",
        params![name],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn test_state() -> AppState {
    AppState::new(conn(), ":memory:", crate::infra::testpaths::scratch("media"))
}

#[test]
fn 建卡读回正文与示例列表() {
    let conn = conn();
    let id = create(&conn, CardKind::Character, &input("Shinku")).unwrap();
    let got = get(&conn, CardKind::Character, id).unwrap().unwrap();
    assert_eq!(
        got.prompt.as_deref(),
        Some("A quiet girl with red eyes and silver hair.")
    );
    assert_eq!(got.negative.as_deref(), Some("blurry, low quality"));
    assert!(got.examples.is_empty(), "新卡没有示例");
    let json = serde_json::to_value(&got).unwrap();
    assert!(json.get("examples").unwrap().is_array());
    assert!(json.get("prompt").is_some());
    assert!(
        json.get("tagPrompt").is_none(),
        "卡片只有一版正文，tag 字段不许再出现：{json}"
    );
}

#[test]
fn 空字段落_null_而不是空串() {
    let conn = conn();
    let card = CardInput {
        name: "空卡".into(),
        prompt: Some("   ".into()),
        negative: Some("\n".into()),
    };
    let id = create(&conn, CardKind::Style, &card).unwrap();
    let got = get(&conn, CardKind::Style, id).unwrap().unwrap();
    assert!(got.prompt.is_none() && got.negative.is_none());
    let json = card_json(&got);
    assert!(json.get("prompt").is_none(), "整键省略：{json}");
    assert!(json.get("negative").is_none(), "整键省略：{json}");
}

#[test]
fn 同名合并不抹掉这次没给的字段() {
    let conn = conn();
    let (id1, created1) = save(&conn, CardKind::Character, input("Iroi")).unwrap();
    assert!(created1);

    // 第二轮只补正文：负面提示词必须原样保留
    let second = CardInput {
        name: "iroi".into(),
        prompt: Some("Iroi, green hair and sheep horns, standing under a lamp.".into()),
        negative: None,
    };
    let (id2, created2) = save(&conn, CardKind::Character, second).unwrap();
    assert!(!created2, "同名必须走合并");
    assert_eq!(id1, id2);

    let got = get(&conn, CardKind::Character, id1).unwrap().unwrap();
    assert_eq!(
        got.negative.as_deref(),
        Some("blurry, low quality"),
        "没给负面时不许抹掉"
    );
    assert!(got.prompt.unwrap().contains("green hair"));

    // 第三轮只改负面：正文必须原样保留
    let third = CardInput {
        name: "iroi".into(),
        prompt: None,
        negative: Some("lowres".into()),
    };
    save(&conn, CardKind::Character, third).unwrap();
    let got = get(&conn, CardKind::Character, id1).unwrap().unwrap();
    assert_eq!(got.negative.as_deref(), Some("lowres"));
    assert!(got.prompt.unwrap().contains("green hair"));
}

#[test]
fn 正文是整段替换不是追加() {
    let conn = conn();
    let (id, _) = save(&conn, CardKind::Character, input("Shinku")).unwrap();
    let second = CardInput {
        name: "Shinku".into(),
        prompt: Some("Now with blue eyes.".into()),
        negative: None,
    };
    save(&conn, CardKind::Character, second).unwrap();
    let got = get(&conn, CardKind::Character, id).unwrap().unwrap();
    assert_eq!(got.prompt.as_deref(), Some("Now with blue eyes."));
}

#[test]
fn 全量替换会清掉没给的字段() {
    let conn = conn();
    let id = create(&conn, CardKind::Style, &input("赛博国风")).unwrap();
    let replaced = CardInput {
        name: "赛博国风".into(),
        ..Default::default()
    };
    assert!(update(&conn, CardKind::Style, id, &replaced).unwrap());
    let got = get(&conn, CardKind::Style, id).unwrap().unwrap();
    assert!(got.prompt.is_none() && got.negative.is_none());
}

#[test]
fn 字段超长报错而不是截断() {
    let conn = conn();
    let long = "x".repeat(MAX_PROMPT_CHARS + 1);
    let card = CardInput {
        name: "x".into(),
        prompt: Some(long.clone()),
        negative: None,
    };
    let err = create(&conn, CardKind::Style, &card).unwrap_err();
    let message = format!("{err:?}");
    assert!(message.contains("正面提示词 超出上限"), "{message}");
    assert!(message.contains("不要指望被悄悄截断"), "{message}");

    let card = CardInput {
        name: "x".into(),
        prompt: None,
        negative: Some(long),
    };
    let err = create(&conn, CardKind::Character, &card).unwrap_err();
    assert!(format!("{err:?}").contains("负面提示词 超出上限"));
    assert!(create(&conn, CardKind::Character, &CardInput::default()).is_err());
    let long_name = "名".repeat(MAX_NAME_CHARS + 1);
    assert!(create(&conn, CardKind::Character, &CardInput { name: long_name, ..Default::default() }).is_err());
}

#[test]
fn 搜索命中正文与负面() {
    let conn = conn();
    create(&conn, CardKind::Character, &input("Shinku")).unwrap();
    create(
        &conn,
        CardKind::Character,
        &CardInput {
            name: "Iroi".into(),
            prompt: Some("Sheep horns under a street lamp.".into()),
            negative: Some("green skin".into()),
        },
    )
    .unwrap();

    assert_eq!(list(&conn, CardKind::Character, None, MAX_SEARCH).unwrap().len(), 2);
    assert_eq!(list(&conn, CardKind::Character, Some("shin"), MAX_SEARCH).unwrap().len(), 1);
    assert_eq!(
        list(&conn, CardKind::Character, Some("sheep horns"), MAX_SEARCH).unwrap().len(),
        1,
        "正文要命中"
    );
    assert_eq!(
        list(&conn, CardKind::Character, Some("green skin"), MAX_SEARCH).unwrap().len(),
        1,
        "负面提示词要命中"
    );
    assert!(list(&conn, CardKind::Character, Some("不存在"), MAX_SEARCH).unwrap().is_empty());
}

#[test]
fn 角色卡与画风卡同名也是两条记录() {
    let conn = conn();
    let character = create(&conn, CardKind::Character, &input("月光")).unwrap();
    let style = create(
        &conn,
        CardKind::Style,
        &CardInput {
            name: "月光".into(),
            prompt: Some("A cool moonlight palette with soft edges.".into()),
            negative: None,
        },
    )
    .unwrap();
    // 两张表各自自增，id 可以撞（都是 1）—— **种类 + id** 才是主键。
    let c = get(&conn, CardKind::Character, character).unwrap().unwrap();
    let s = get(&conn, CardKind::Style, style).unwrap().unwrap();
    assert!(c.prompt.unwrap().contains("red eyes"));
    assert!(s.prompt.unwrap().contains("moonlight"));
    assert_eq!(
        list(&conn, CardKind::Character, Some("月光"), MAX_SEARCH).unwrap().len(),
        1
    );
    assert_eq!(list(&conn, CardKind::Style, Some("月光"), MAX_SEARCH).unwrap().len(), 1);
}

#[test]
fn 示例产物保持加入顺序_并且不能超限() {
    let conn = conn();
    let id = create(&conn, CardKind::Style, &input("赛博国风")).unwrap();
    let a = media(&conn, "a.png");
    let b = media(&conn, "b.png");
    let c = media(&conn, "c.png");

    let examples = replace_examples(&conn, CardKind::Style, id, &[c, a, b, a]).unwrap();
    assert_eq!(examples.iter().map(|e| e.id).collect::<Vec<_>>(), vec![c, a, b], "去重且保持顺序");
    assert_eq!(examples[0].file_url, format!("/api/media/{c}/file"));
    assert_eq!(examples[0].thumb_url, format!("/api/media/{c}/thumb"));

    // 追加不会重复插入
    add_examples(&conn, CardKind::Style, id, &[b, c]).unwrap();
    assert_eq!(list_examples(&conn, CardKind::Style, id).unwrap().len(), 3);

    // 上限：报错而不是截断
    let ids: Vec<i64> = (0..=MAX_EXAMPLES).map(|i| media(&conn, &format!("x{i}.png"))).collect();
    let err = replace_examples(&conn, CardKind::Style, id, &ids).unwrap_err();
    assert!(format!("{err:?}").contains("最多"), "{err:?}");

    // 不存在的产物 id 要如实拒绝
    let err = replace_examples(&conn, CardKind::Style, id, &[999999]).unwrap_err();
    assert!(format!("{err:?}").contains("999999"), "{err:?}");
}

#[test]
fn 删产物会从卡片示例里自动摘掉() {
    let conn = conn();
    let id = create(&conn, CardKind::Character, &input("Shinku")).unwrap();
    let a = media(&conn, "a.png");
    let b = media(&conn, "b.png");
    replace_examples(&conn, CardKind::Character, id, &[a, b]).unwrap();
    conn.execute("DELETE FROM media_assets WHERE id = ?1", params![a]).unwrap();
    let left = list_examples(&conn, CardKind::Character, id).unwrap();
    assert_eq!(left.iter().map(|e| e.id).collect::<Vec<_>>(), vec![b], "外键级联要摘掉");
}

#[test]
fn 工具_正文空着就明说() {
    let state = test_state();
    let first = tool_save(
        &state,
        CardKind::Character,
        &serde_json::json!({ "name": "Shinku" }),
    )
    .unwrap();
    assert!(first.content.contains("已保存角色卡 #1「Shinku」（新建）"), "{}", first.content);
    assert!(first.content.contains("正面提示词:（空）"), "{}", first.content);
    assert!(first.content.contains("还空着**正面提示词**"), "{}", first.content);
    assert!(first.content.contains("600+ token"), "{}", first.content);

    // 第二轮补正文：负面提示词不许被抹掉
    let second = tool_save(
        &state,
        CardKind::Character,
        &serde_json::json!({
            "name": "shinku",
            "prompt": "A girl with red eyes, head to toe, layered clothing, every detail described."
        }),
    )
    .unwrap();
    assert!(second.content.contains("（合并更新）"), "{}", second.content);
    assert!(second.payload["prompt"].as_str().unwrap().contains("head to toe"));
    assert!(!second.content.contains("还空着"), "{}", second.content);

    // 画风卡同样会提醒（文案里给出 100~200 token 的目标）
    let style = tool_save(&state, CardKind::Style, &serde_json::json!({ "name": "赛博国风" })).unwrap();
    assert!(style.content.contains("已保存画风卡"), "{}", style.content);
    assert!(style.content.contains("100~200 token"), "{}", style.content);
}

#[test]
fn 工具_save_可以顺手挂示例产物() {
    let conn = conn();
    let media_id = media(&conn, "example.png");
    let state = AppState::new(conn, ":memory:", crate::infra::testpaths::scratch("media"));

    let saved = tool_save(
        &state,
        CardKind::Style,
        &serde_json::json!({
            "name": "赛博国风",
            "prompt": "semi-thick line work, low-chroma blue-gray field, warm islands, luminous finish",
            "exampleMediaIds": [media_id]
        }),
    )
    .unwrap();
    assert_eq!(saved.payload["examples"][0]["id"], media_id);
    assert!(saved.content.contains("示例产物（1）"), "{}", saved.content);

    // 非整数项要报错，不许静默丢掉（否则 AI 以为挂上了、界面却没有）
    let err = tool_save(
        &state,
        CardKind::Style,
        &serde_json::json!({ "name": "另一张", "exampleMediaIds": [media_id, "oops"] }),
    )
    .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(err.message.contains("oops"), "{}", err.message);
}

#[test]
fn 工具_搜索查不到时给对下一步() {
    let state = test_state();
    let err = tool_search(&state, CardKind::Style, &serde_json::json!({ "query": "厚涂" })).unwrap_err();
    assert_eq!(err.code, "NOT_FOUND");
    assert!(err.message.contains("画风卡"), "{}", err.message);
    assert!(err.message.contains("save_style_card"), "{}", err.message);
    assert!(err.message.contains("100~200 token"), "{}", err.message);

    let err = tool_search(&state, CardKind::Character, &serde_json::json!({ "query": "shinku" }))
        .unwrap_err();
    assert!(err.message.contains("save_character_card"), "{}", err.message);
    assert!(err.message.contains("danbooru_lookup"), "{}", err.message);
}

#[test]
fn 正文形状把正文与示例都画出来() {
    let conn = conn();
    let id = create(&conn, CardKind::Character, &input("Shinku")).unwrap();
    let sample = media(&conn, "sample.png");
    replace_examples(&conn, CardKind::Character, id, &[sample]).unwrap();
    let card = get(&conn, CardKind::Character, id).unwrap().unwrap();
    let text = render_card(CardKind::Character, &card);
    assert!(text.contains("#1「Shinku」（角色卡）"), "{text}");
    assert!(text.contains("正面提示词: A quiet girl"), "{text}");
    assert!(text.contains("负面提示词: blurry, low quality"), "{text}");
    assert!(text.contains(&format!("示例产物（1）: #{sample}「sample.png」")), "{text}");
    assert!(!text.contains("tag"), "卡片正文不许再出现 tag 版：{text}");

    let empty = CardDto {
        id: 2,
        name: "空的".into(),
        prompt: None,
        negative: None,
        created_at: None,
        updated_at: None,
        examples: Vec::new(),
    };
    let text = render_card(CardKind::Style, &empty);
    assert!(text.contains("正面提示词:（空）"), "{text}");
    assert!(text.contains("示例产物:（无）"), "{text}");
}

#[test]
fn 直接建同名卡是_400_那一档而不是_500() {
    let conn = conn();
    create(&conn, CardKind::Style, &input("赛博国风")).unwrap();
    let err = create(&conn, CardKind::Style, &input("赛博国风")).unwrap_err();
    assert!(matches!(err, WriteError::Conflict(_)), "同名的 POST 要能被界面理解成 400：{err:?}");
    let message = format!("{err:?}");
    assert!(message.contains("同名画风卡已经存在"), "{message}");
}

#[test]
fn 删除与不存在的行为() {
    let conn = conn();
    let id = create(&conn, CardKind::Character, &input("Shinku")).unwrap();
    assert!(delete(&conn, CardKind::Character, id).unwrap());
    assert!(!delete(&conn, CardKind::Character, id).unwrap(), "删第二次要如实说没删到");
    assert!(get(&conn, CardKind::Character, id).unwrap().is_none());
    assert!(!update(&conn, CardKind::Character, id, &input("Shinku")).unwrap());
}
