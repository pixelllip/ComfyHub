//! `media` 的单元测试（主体代码在 [`media`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn range_parsing_covers_the_three_shapes() {
    assert_eq!(parse_range("bytes=0-499", 1000), Some((0, 499)));
    assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
    assert_eq!(parse_range("bytes=-200", 1000), Some((800, 999)));
    // 越界 / 多段 / 畸形一律不当 Range（回 200 全量，而不是错格式的 206）
    assert_eq!(parse_range("bytes=1000-2000", 1000), None);
    assert_eq!(parse_range("bytes=0-1,5-6", 1000), None);
    assert_eq!(parse_range("items=0-1", 1000), None);
    assert_eq!(parse_range("bytes=-0", 0), None);
}

#[test]
fn path_traversal_is_rejected() {
    let dir = std::path::Path::new("D:/x/storage/media");
    assert!(normalize_path(&dir.join("../secret.txt")).is_some());
    let resolved = normalize_path(&dir.join("../secret.txt")).unwrap();
    assert!(!resolved.starts_with(dir), "越界必须被 starts_with 拦住");
    assert!(resolve_media(dir, "../secret.txt").is_none());
    assert!(resolve_media(dir, "").is_none());
}

#[test]
fn inline_disposition_carries_a_utf8_fallback() {
    let value = inline_disposition("赛博朋克.png");
    assert!(value.contains("filename=\"______.png\"") || value.contains("filename=\""));
    assert!(value.contains("filename*=UTF-8''"));
}

#[test]
fn tag_param_is_normalized_and_deduped() {
    assert_eq!(
        parse_tag_param(Some(" 赛博 , 国风,赛博")),
        vec!["赛博", "国风"]
    );
    assert!(parse_tag_param(None).is_empty());
}
