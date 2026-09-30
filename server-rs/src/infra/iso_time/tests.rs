//! `iso_time` 的单元测试（主体代码在 [`iso_time`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

/// Java `Instant.toString()` 的小数位规则（三位一组去尾零），直接钉死在这条用例上。
#[test]
fn java_instant_fraction_digits_are_grouped_by_three() {
    // 整秒：**没有小数点**
    assert_eq!(format(1_774_512_000, 0), "2026-03-26T08:00:00Z");
    assert_eq!(format(1_774_512_000, 1_000_000), "2026-03-26T08:00:00.001Z", "1ms → 3 位");
    assert_eq!(format(1_774_512_000, 1_000), "2026-03-26T08:00:00.000001Z", "1µs → 6 位");
    assert_eq!(format(1_774_512_000, 200), "2026-03-26T08:00:00.000000200Z", "200ns → 9 位");
    // Windows 的 FileTime 精度是 100ns：实测真机上就是这个形状
    assert_eq!(format(1_774_512_000, 917_303_200), "2026-03-26T08:00:00.917303200Z");
}

/// 往返：`format` 出来的东西必须能被 `parse` 读回去（小数位规则不是单向的）。
#[test]
fn parse_and_format_round_trip() {
    for raw in [
        "2026-09-23T13:45:59Z",
        "2026-09-23T13:45:59.287Z",
        "2026-09-23T13:45:59.917303200Z",
        "2026-09-23 13:45:59",
        "2026-09-23T21:45:59+08:00",
        "2026-09-23T05:45:59-08:00",
    ] {
        let parsed = parse(raw).unwrap_or_else(|| panic!("解析不了 {raw}"));
        let out = format(parsed.0, parsed.1);
        assert_eq!(parse(&out), Some(parsed), "{raw} → {out} 不能往返");
    }
}

/// ★ 这条就是"定时炸弹"本身：库里的 `.000Z` 在 Kotlin 那边发出来**没有小数点**。
#[test]
fn dot_zero_collapses_like_kotlin() {
    assert_eq!(
        iso_time(Some("2026-09-23T13:45:59.000Z")).unwrap(),
        "2026-09-23T13:45:59Z"
    );
    // 老 MySQL 的空格文本按 UTC 解释（SQLite 的 CURRENT_TIMESTAMP 也是这个形状）
    assert_eq!(
        iso_time(Some("2026-09-23 13:45:59")).unwrap(),
        "2026-09-23T13:45:59Z"
    );
    // 带偏移要换算成 UTC
    assert_eq!(
        iso_time(Some("2026-09-23T21:45:59+08:00")).unwrap(),
        "2026-09-23T13:45:59Z"
    );
}

/// 认不出来就是 `None`（= Kotlin 的 null = 整个键不出现），**不许原样透传**。
#[test]
fn unparseable_becomes_none_not_passthrough() {
    for bad in [
        None,
        Some(""),
        Some("   "),
        Some("不是时间"),
        Some("2026-09-23"),
        Some("2026-09-23T13:45"),
        Some("2026-09-23T25:45:59Z"),
        Some("2026-09-23T13:45:59+8:00"),
        Some("2026/09/23 13:45:59"),
    ] {
        assert_eq!(iso_time(bad), None, "{bad:?} 应当解析失败");
    }
}
