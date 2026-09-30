//! `conversations` 的单元测试（主体代码在 [`conversations`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

/// 库里的 NULL 在 DTO 上的**回落值**（Kotlin `toConversation` / `toMessage` 的那几个 `?:`）。
///
/// ⚠ 用"算出来的行"测，而不是夹具：真 schema 上 `title` / `system_prompt_version` /
/// `status` 都是 NOT NULL，夹具里**构造不出** NULL —— 只靠集成测试的话，
/// 这几条回落路径等于没有防线（老 MySQL 搬过来的脏行才走得到）。
#[test]
fn 可空列在_dto_上都有回落值() {
    let conn = crate::db::open_memory().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT 'c1' AS id, NULL AS title, NULL AS provider_id, NULL AS model_id,
                    NULL AS system_prompt_version, NULL AS archived, NULL AS message_count,
                    NULL AS created_at, NULL AS updated_at",
        )
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    let conv = conversation_from_row(rows.next().unwrap().unwrap()).unwrap();
    assert_eq!(conv.title, "新对话", "标题空着就回落成「新对话」，不是空串");
    assert_eq!(conv.system_prompt_version, "v1");
    assert!(!conv.archived);
    assert_eq!(conv.message_count, 0, "取不到 message_count 就当 0（那两条 SQL 之外的行）");
    assert!(conv.created_at.is_none() && conv.updated_at.is_none());

    let mut stmt = conn
        .prepare(
            "SELECT 'm1' AS id, 'c1' AS conversation_id, NULL AS seq, NULL AS role,
                    NULL AS status, NULL AS text, NULL AS provider_id, NULL AS model_id,
                    NULL AS provider_response_id, NULL AS usage_json,
                    NULL AS created_at, NULL AS updated_at",
        )
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    let msg = message_from_row(rows.next().unwrap().unwrap()).unwrap();
    assert_eq!(msg.status, "complete");
    assert_eq!(msg.text, "");
    assert_eq!(msg.seq, 0);
    assert!(msg.parts.is_empty());
}

/// 脏 `json_payload` / `usage_json` → **当没有**（键不出现），不抛。
#[test]
fn 坏的_json_列按没有处理() {
    assert_eq!(parse_json_any(Some("not json")), None);
    assert_eq!(parse_json_any(Some("  ")), None);
    assert_eq!(parse_json_any(None), None);
    // usage 只认对象：数组 / 标量一概当没有（Kotlin 那边 `runCatching` 能拿到数组，
    // 但那个列本来只会写对象；这里收紧成"不是对象就没有"，免得界面上出现一个读不懂的字段）
    assert_eq!(parse_json_object(Some("[1,2]")), None);
    assert_eq!(parse_json_object(Some("12")), None);
    assert!(parse_json_object(Some(r#"{"a":1}"#)).is_some());
}
