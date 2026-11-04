//! `questions` 的单元测试（主体代码在 [`questions`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

/// `askedAt` 是**数字毫秒**（与 `/api/ai/pending` 里审批那份的 ISO 文本刻意不同）。
#[test]
fn asked_at_是毫秒而脏数据回零() {
    assert_eq!(asked_at_ms(Some("2026-09-24T14:41:29.647Z")), 1_790_260_889_647);
    assert_eq!(asked_at_ms(Some("2026-09-24T14:41:29Z")), 1_790_260_889_000);
    assert_eq!(asked_at_ms(Some("不是时间")), 0);
    assert_eq!(asked_at_ms(None), 0);
}

/// 可空键**整个键不出现**（`explicitNulls = false`）—— 多一个 `"header": null`
/// 就是与 Kotlin 的形状不一致，而客户端不会报错，只会画出一块空白。
#[test]
fn 可空键不出现而_group_id_空则回落到自己() {
    let q = StoredQuestion {
        question_id: "q1".into(),
        run_id: "r1".into(),
        conversation_id: "c1".into(),
        group_id: String::new(),
        index: 0,
        total: 1,
        question: "要哪个画幅？".into(),
        options: vec!["4:3".into(), "16:9".into()],
        option_images: Vec::new(),
        image_url: None,
        allow_free_text: true,
        multi_select: false,
        header: None,
        status: "pending".into(),
        answer: None,
        option_index: None,
        option_indexes: Vec::new(),
        asked_at_ms: 1_790_260_889_647,
    };
    let json = q.to_json();
    let keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
    for absent in [
        "header",
        "answer",
        "optionIndex",
        "optionIndexes",
        "imageUrl",
        "optionImages",
    ] {
        assert!(!keys.contains(&absent), "{absent} 不该出现：{json}");
    }
    assert_eq!(json["groupId"], "q1", "空 group_id 回落到 questionId（与 Kotlin 同）");
    assert_eq!(json["askedAt"], 1_790_260_889_647_i64);
    assert_eq!(json["answered"], false);

    // 答完之后：answer / optionIndex 必须出现
    let answered = q.with_answer(&Answer {
        answer: "4:3".into(),
        option_index: Some(0),
        option_indexes: Vec::new(),
    });
    let json = answered.to_json();
    assert_eq!(json["answered"], true);
    assert_eq!(json["answer"], "4:3");
    assert_eq!(json["optionIndex"], 0);
    assert!(json.as_object().unwrap().get("optionIndexes").is_none(), "空的选项数组也不出现");
}

#[test]
fn 带图提问的图片字段只在真有图时出现() {
    let mut q = StoredQuestion {
        question_id: "q2".into(),
        run_id: "r2".into(),
        conversation_id: "c2".into(),
        group_id: "g2".into(),
        index: 0,
        total: 1,
        question: "刘海是哪一种？".into(),
        options: vec!["齐刘海".into(), "斜刘海".into()],
        option_images: vec![
            "/api/ai/attachments/a/file".into(),
            String::new(),
        ],
        image_url: Some("/api/ai/attachments/crop/file".into()),
        allow_free_text: true,
        multi_select: false,
        header: Some("发型".into()),
        status: "pending".into(),
        answer: None,
        option_index: None,
        option_indexes: Vec::new(),
        asked_at_ms: 0,
    };
    let json = q.to_json();
    assert_eq!(json["imageUrl"], "/api/ai/attachments/crop/file");
    assert_eq!(json["optionImages"][0], "/api/ai/attachments/a/file");
    assert_eq!(json["optionImages"][1], "", "占位空串照原样保留：下标必须与选项对齐");
    assert_eq!(json["options"][1], "斜刘海");

    // 全空时整两个键都不出现（老形状不变）
    q.option_images = vec![String::new(), String::new()];
    q.image_url = None;
    let json = q.to_json();
    assert!(json.as_object().unwrap().get("optionImages").is_none());
    assert!(json.as_object().unwrap().get("imageUrl").is_none());
}

#[test]
fn 脏的_options_json_当没有() {
    assert!(parse_string_array_str(Some("not json")).is_empty());
    assert_eq!(parse_string_array_str(Some(r#"["a","b"]"#)), vec!["a", "b"]);
    let idx = parse_int_array(Some("[0,2]"));
    assert_eq!(idx, vec![Value::from(0), Value::from(2)]);
    assert!(parse_int_array(Some(r#"["x"]"#)).is_empty());
}
