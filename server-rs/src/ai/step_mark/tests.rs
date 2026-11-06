//! `step_mark` 的单元测试（主体代码在 [`step_mark`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn feed(stripper: &mut StepStripper, chunks: &[&str]) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut phases = Vec::new();
    for chunk in chunks {
        let (t, p) = stripper.push(chunk);
        text.push_str(&t);
        phases.extend(p);
    }
    (text, phases)
}

#[test]
fn 标记摘掉_正文一个字不少_环节按出现顺序() {
    let mut stripper = StepStripper::new();
    let (text, phases) = feed(
        &mut stripper,
        &["[步骤]正在分析请求[/步骤]我先看看需求。[步骤]正在读文件[/步骤]读一下工作流。"],
    );
    assert_eq!(
        text, "我先看看需求。读一下工作流。",
        "标记本身不该出现在正文里"
    );
    assert_eq!(
        phases,
        vec!["正在分析请求".to_string(), "正在读文件".to_string()]
    );
}

#[test]
fn 标记被切成好几片也不漏半个() {
    let mut stripper = StepStripper::new();
    let (text, phases) = feed(
        &mut stripper,
        &["前", "[步", "骤]正在查", "资料[/步", "骤]", "后"],
    );
    assert_eq!(text, "前后");
    assert_eq!(phases, vec!["正在查资料".to_string()]);
}

/// 名字不在固定枚举里：**摘掉标记，但不记环节**（界面上不该冒出我们没定义的环节名）。
#[test]
fn 不在枚举里的名字摘掉但不记环节() {
    let mut stripper = StepStripper::new();
    let (text, phases) = feed(&mut stripper, &["[步骤]正在玩耍[/步骤]接着干"]);
    assert_eq!(text, "接着干", "标记不许漏进正文");
    assert!(phases.is_empty(), "枚举外的名字不许变成环节：{phases:?}");
    // 空白/空格差异要能对上（模型偶尔会写成「正在 查资料」）
    let (_, phases) = feed(&mut stripper, &["[步骤]正在 查资料[/步骤]"]);
    assert_eq!(phases, vec!["正在查资料".to_string()]);
}

/// 不按格式来时**一个字都不能吞**：不闭合的标记、撑爆的标记、flush 都要放行原文。
#[test]
fn 不按格式来时一个字都不吞() {
    // ① 一直不闭合、且长度超限 → 连 `[步骤]` 一起当正文放出去
    let mut stripper = StepStripper::new();
    let long = format!("[步骤]{}", "长".repeat(MAX_PHASE_CHARS + 50));
    let (text, phases) = stripper.push(&long);
    assert!(text.contains("长"), "撑爆的标记要当正文放出去：{text}");
    assert!(text.starts_with(OPEN), "原文一个字都不能少：{text}");
    assert!(phases.is_empty());

    // ② 没到上限就先压着（等下一片），flush 时必须放出来
    let mut stripper = StepStripper::new();
    let (text, _) = stripper.push("[步骤]正在查");
    assert_eq!(text, "", "还没判定完，先压着");
    let flushed = stripper.flush();
    assert_eq!(flushed, "[步骤]正在查", "流结束时必须原样放出来");

    // ③ 普通方括号（不是标记）立刻放行，不会因为一个 `[` 就把后面的正文按住
    let mut stripper = StepStripper::new();
    let (text, phases) = stripper.push("看 [1] 这张图");
    assert_eq!(text, "看 [1] 这张图");
    assert!(phases.is_empty());
}

#[test]
fn 一条增量里连着好几个标记() {
    let mut stripper = StepStripper::new();
    let (text, phases) = stripper.push("[步骤]正在分析请求[/步骤][步骤]正在写文件[/步骤]写完了");
    assert_eq!(text, "写完了");
    assert_eq!(
        phases,
        vec!["正在分析请求".to_string(), "正在写文件".to_string()]
    );
}

#[test]
fn 六个环节名都在枚举里() {
    for phase in PHASES {
        assert_eq!(normalize_phase(phase).as_deref(), Some(phase));
    }
    assert_eq!(PHASES.len(), 6, "系统提示里写死的也是这六个");
}
