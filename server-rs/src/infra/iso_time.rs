//! ISO-8601 时间的**解析 + Java 同形输出**（P4 第 21 步）。
//!
//! # 为什么需要它
//!
//! Kotlin 侧的时间列统一走 `ResultSet.isoTime(col)`（`Db.kt:380`）=
//! `parseDbInstant(...)?.toString()` —— 也就是
//! **先解析再按 Java `Instant.toString()` 的规则重新格式化**，解析不出来就 `null`
//! （`explicitNulls = false` → 整个键不出现）。
//!
//! 也就是说它不是"原样透传"：
//!
//! | 库里存的文本 | Kotlin 发出去的 |
//! | --- | --- |
//! | `2026-09-23T13:45:59.287Z` | `2026-09-23T13:45:59.287Z`（同） |
//! | `2026-09-23T13:45:59.000Z` | **`2026-09-23T13:45:59Z`**（小数位为 0 → 小数点整个不出现） |
//! | `2026-09-23 13:45:59`（老 MySQL 文本） | `2026-09-23T13:45:59Z`（按 UTC 解释） |
//! | `2026-09-23T21:45:59+08:00` | `2026-09-23T13:45:59Z`（换算成 UTC） |
//! | 认不出来的脏数据 | **键不出现** |
//!
//! Rust 侧早期几批是**直接透传那一列**的。本机真库上两种写法结果一致
//! （实测 27 提示词 / 24 产物 / 23 会话 / 144 消息里**没有一个** `.000Z`、也没有一个非 ISO 文本），
//! 因此那不是"已经在线上错了"，而是一处**潜在缺陷**：`strftime('%Y-%m-%dT%H:%M:%fZ','now')`
//! 的毫秒位是 `000` 的概率是 1/1000 —— 哪天撞上，两端就悄悄差一个字符串。
//! 所以这个模块是**时间列的唯一出口**，新写的 DTO 一律用它。

use std::time::{SystemTime, UNIX_EPOCH};

/// 按 Java `Instant.toString()` 的规则格式化（小数位三位一组、最少 0 位）。
///
/// 规则（`DateTimeFormatter.ISO_INSTANT` 的实际行为）：
/// 整秒**不带小数点**；否则毫秒 3 位、微秒 6 位、纳秒 9 位，且**按三位一组去掉尾部零**。
/// 这就是"真机 Kotlin 给出 `…:33.917303200Z`、我们曾只给 `…:917Z`"该问题的答案。
pub fn format(secs: i64, nanos: u32) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days);
    if nanos == 0 {
        return format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z");
    }
    let mut frac = format!("{nanos:09}");
    while frac.len() > 3 && frac.ends_with("000") {
        frac.truncate(frac.len() - 3);
    }
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{frac}Z")
}

/// `SystemTime` → ISO-8601（`comfy_dirs` 的工作流文件时间用它）。
pub fn from_system_time(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    format(d.as_secs() as i64, d.subsec_nanos())
}

/// **认得的几种时间文本 → `(epoch 秒, 纳秒)`**；认不出来一律 `None`。
///
/// 与 Kotlin `parseDbInstant` 认的三种形态对齐：
/// `Instant.parse`（`…Z`）、`OffsetDateTime.parse`（带偏移）、
/// 空格分隔的 `LocalDateTime`（**按 UTC 解释**，那是 SQLite `CURRENT_TIMESTAMP` 的形状）。
pub fn parse(raw: &str) -> Option<(i64, u32)> {
    let t = raw.trim();
    let b = t.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || !(b[10] == b'T' || b[10] == b't' || b[10] == b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { t.get(from..to)?.parse::<i64>().ok() };
    let year = num(0, 4)?;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    let hour = num(11, 13)?;
    let minute = num(14, 16)?;
    let second = num(17, 19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    // 小数秒（最多 9 位有效；多出来的直接丢，与 Java 的纳秒精度一致）
    let mut rest = &t[19..];
    let mut nanos: u32 = 0;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits: String = after_dot
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            return None;
        }
        let mut padded = digits.clone();
        padded.truncate(9);
        while padded.len() < 9 {
            padded.push('0');
        }
        nanos = padded.parse().ok()?;
        rest = &after_dot[digits.len()..];
    }

    // 时区：`Z` / 无 / `±HH:MM`
    let offset = match rest {
        "" | "Z" | "z" => 0,
        other => {
            let sign = match other.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            if other.as_bytes().get(3) != Some(&b':') {
                return None;
            }
            let hh: i64 = other.get(1..3)?.parse().ok()?;
            let mm: i64 = other.get(4..6)?.parse().ok()?;
            if other.len() != 6 || hh > 23 || mm > 59 {
                return None;
            }
            sign * (hh * 3600 + mm * 60)
        }
    };

    let secs = days_from_civil(year, month as u32, day as u32) * 86_400
        + hour * 3600
        + minute * 60
        + second
        - offset;
    Some((secs, nanos))
}

/// 时间列 → Java 同形 ISO 字符串（`ResultSet.isoTime` 的对应物）。
///
/// **解析不出来就是 `None`** —— 那正是 Kotlin 回 `null`、整个键不出现的语义。
/// 不要在这里"兜底成原样透传"：那会让一个坏值以两种形状出现在两端。
pub fn iso_time(raw: Option<&str>) -> Option<String> {
    let text = raw?.trim();
    if text.is_empty() {
        return None;
    }
    parse(text).map(|(secs, nanos)| format(secs, nanos))
}

/// 天数 → 公历（Howard Hinnant 的 `civil_from_days`，无闰秒、无时区表）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 公历 → 天数（`days_from_civil`，`civil_from_days` 的逆）。
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = ((m + 9) % 12) as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

#[cfg(test)]
mod tests;
