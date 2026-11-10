//! `title_strip` 的单元测试（主体代码在 [`title_strip`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn feed(stripper: &mut TitleStripper, chunks: &[&str]) -> String {
    chunks.iter().map(|c| stripper.push(c)).collect()
}

#[test]
fn 标记被摘掉_标题写出来_正文一个字不少() {
    let mut stripper = TitleStripper::new(true);
    let text = feed(
        &mut stripper,
        &["[标题]Krea2 画风迁移[/标题]\n\n你好，我可以帮你。"],
    );
    assert_eq!(text, "你好，我可以帮你。", "标记与它后面的空行都不该出现");
    assert_eq!(stripper.title(), Some("Krea2 画风迁移"));
    assert!(stripper.finished());
}

#[test]
fn 标记被切成好几片也不漏半个() {
    let mut stripper = TitleStripper::new(true);
    let text = feed(
        &mut stripper,
        &["[标", "题]海", "报设计[/标", "题]", "开始吧"],
    );
    assert_eq!(text, "开始吧");
    assert_eq!(stripper.title(), Some("海报设计"));
}

#[test]
fn 不按格式来时一个字都不吞() {
    let mut stripper = TitleStripper::new(true);
    // 压过 MAX_START_LOOKAHEAD 才放行（说明模型根本不打算给标题）
    let long = "啰".repeat(MAX_START_LOOKAHEAD + 5);
    let text = stripper.push(&long);
    assert_eq!(
        text.chars().count(),
        long.chars().count(),
        "压久了必须全放出来"
    );
    assert!(!stripper.finished(), "还没确认没有标记");
    // 标题一直不闭合：长度超限后当正文放行
    let mut stripper = TitleStripper::new(true);
    let text = stripper.push(&format!("[标题]{}", "长".repeat(MAX_TITLE_CHARS + 50)));
    assert!(text.contains('长'), "不闭合的标题要当正文放出去：{text}");
    // flush 兜底：没闭合的缓冲一律放行
    let mut stripper = TitleStripper::new(true);
    let partial = stripper.push("[标题]只有一半");
    assert_eq!(partial, "", "还没到认输的长度，先压着");
    let flushed = stripper.flush();
    assert!(
        flushed.contains("只有一半"),
        "流结束时必须放出来：{flushed}"
    );
}

#[test]
fn 回声只摘标记_不改已定的标题() {
    let mut stripper = TitleStripper::new(true);
    let first = feed(&mut stripper, &["[标题]一号[/标题]正文开始"]);
    assert_eq!(first, "正文开始");
    assert_eq!(stripper.title(), Some("一号"));
    // 模型在后面的轮次里又吐了一遍标记（真机实测过）
    let second = stripper.push("还有一段[标题]二号[/标题]接着写");
    assert_eq!(second, "还有一段接着写", "回声标记也要摘掉，但不许吞掉正文");
    assert_eq!(stripper.title(), Some("一号"), "回声不许改会话名");
}

#[test]
fn 标题的清洗去掉前缀与包裹() {
    assert_eq!(
        clean_title("标题：Krea2 出图").as_deref(),
        Some("Krea2 出图")
    );
    assert_eq!(clean_title("《初音未来》").as_deref(), Some("初音未来"));
    assert_eq!(clean_title("多余   空白").as_deref(), Some("多余 空白"));
    // ⚠ 与 Kotlin **逐字一致**的一处"怪"：去掉引号之后**不再 trim 空白**
    //   （Kotlin 的两次 `trim(…)` 里都没有空格）—— 所以引号内的空白会留着。
    //   照抄比"顺手改好"更安全：改了就是两端标题不一样。
    assert_eq!(
        clean_title("\"  多余 空白  \"").as_deref(),
        Some(" 多余 空白 ")
    );
    assert_eq!(clean_title("   "), None);
    assert_eq!(
        clean_title(&"长".repeat(100)).unwrap().chars().count(),
        MAX_TITLE_CHARS
    );
}

#[test]
fn 关掉时原样透传() {
    let mut stripper = TitleStripper::new(false);
    let text = stripper.push("[标题]不该摘[/标题]正文");
    assert_eq!(text, "[标题]不该摘[/标题]正文");
    assert_eq!(stripper.title(), None);
    assert!(stripper.finished());
}
