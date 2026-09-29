//! **关键步骤（环节）标记**的摘取：模型在正文里写 `[步骤]正在查资料[/步骤]`，
//! 后端把它摘成一个**有序块**（`type = "step"`）与一条 `step.started` 事件，
//! 用户看不到这行标记 —— 界面按它把**这个环节里的工具卡折进一组**（用户建议 2026-09-28）。
//!
//! # 为什么是"标记"而不是一个工具
//!
//! 工具调用要走一整轮上游往返（慢、还占工具预算），而"我现在进入哪个环节"是模型本来
//! 就在心里想的事 —— 写成一行标记是免费的。这与"会话标题"用的是同一招
//! （见 [`crate::title_strip`]），风险与纪律也同一套：
//!
//! 1. **绝不吞内容** —— 模型不按格式来时，说过的话一个字都不能少
//!    （标记一直不闭合、名字不合法、缓冲被撑爆，三种情况都把原文当正文放出去）；
//! 2. **可切断** —— 标记会被上游切成好几片（`[步`+`骤]正在查`+`资料[/步骤]`），
//!    所以尾部要留最多"标记长度"那么长的尾巴，等下一片到齐再判；
//! 3. **只有固定枚举才算环节** —— [`PHASES`] 之外的名字**摘掉但不记环节**
//!    （系统提示里写死了这六个；模型要是自己发明一个，界面上不该冒出一个我们自己都没定义的环节名）。
//!    这种情形只留一条日志（`eprintln`），因为"标记进正文"比"少一个分组"更糟。

/// 环节标记。
pub const OPEN: &str = "[步骤]";
pub const CLOSE: &str = "[/步骤]";

/// **固定枚举**（系统提示里逐字给出这六个，`SystemPrompt` 的同一条纪律：
/// 名字与这里必须一致，否则模型写的名字会被当成"不合法的标记"摘掉、界面上什么都不出现）。
pub const PHASES: [&str; 6] = [
    "正在分析请求",
    "正在查资料",
    "正在读文件",
    "正在生成图片",
    "正在写文件",
    "正在收尾",
];

/// 环节名的长度上限（超过就认为这个 `[步骤]` 不是标记，按正文放行）。
pub const MAX_PHASE_CHARS: usize = 24;

/// 摘环节标记的状态机（纯逻辑、可单测）。
#[derive(Default)]
pub struct StepStripper {
    /// 还没判定完的尾巴（可能是标记的一部分，也可能是没闭合的标记）。
    buffer: String,
}

impl StepStripper {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂一片增量 → `(应当进入正文的文本, 依次出现的环节)`。
    ///
    /// 环节是**按出现顺序**给回来的：调用方要按这个顺序封段 + 落块 + 发事件。
    pub fn push(&mut self, chunk: &str) -> (String, Vec<String>) {
        if chunk.is_empty() {
            return (String::new(), Vec::new());
        }
        self.buffer.push_str(chunk);
        self.scan()
    }

    /// 流结束时收尾：没闭合的缓冲一律当正文放出去（**宁可多显示，也不吞内容**）。
    pub fn flush(&mut self) -> String {
        std::mem::take(&mut self.buffer)
    }

    // -----------------------------------------------------------------------

    fn scan(&mut self) -> (String, Vec<String>) {
        let mut out = String::new();
        let mut phases: Vec<String> = Vec::new();
        loop {
            match self.buffer.find(OPEN) {
                None => {
                    // 尾部可能藏着切了一半的标记（`[` / `[步` / `[/步`），留到下一片
                    let keep = partial_marker_start(&self.buffer);
                    out.push_str(&self.drop_before(keep));
                    break;
                }
                Some(open_at) => {
                    out.push_str(&self.drop_before(open_at));
                    // 到这里 buffer 以 OPEN 开头
                    self.buffer.drain(..OPEN.len());
                    match self.buffer.find(CLOSE) {
                        None => {
                            // 还没收全：长度超限就说明这根本不是标记 → 连 OPEN 一起当正文放行
                            if self.buffer.chars().count() > MAX_PHASE_CHARS + 40 {
                                let mut text = OPEN.to_string();
                                text.push_str(&std::mem::take(&mut self.buffer));
                                out.push_str(&text);
                            } else {
                                // 把 OPEN 放回缓冲：等下一片（放回去才能保证"绝不吞内容"）
                                self.buffer.insert_str(0, OPEN);
                            }
                            break;
                        }
                        Some(close_at) => {
                            let raw = self.buffer[..close_at].to_string();
                            self.buffer.drain(..close_at + CLOSE.len());
                            match normalize_phase(&raw) {
                                Some(phase) => phases.push(phase),
                                None => eprintln!(
                                    "环节标记的名字不在固定枚举里（已摘掉、不记环节）：{raw:?}"
                                ),
                            }
                            continue;
                        }
                    }
                }
            }
        }
        (out, phases)
    }

    /// 放行 `[0, end)`，把它们从缓冲里去掉。
    fn drop_before(&mut self, end: usize) -> String {
        if end == 0 {
            return String::new();
        }
        let end = end.min(self.buffer.len());
        let out = self.buffer[..end].to_string();
        self.buffer.drain(..end);
        out
    }
}

/// 缓冲末尾那段"可能还没收全的标记"从哪个下标开始（`[` / `[步` / `[/步` 的前缀）。
fn partial_marker_start(buffer: &str) -> usize {
    let Some(bracket) = buffer.rfind('[') else { return buffer.len() };
    let tail = &buffer[bracket..];
    let count = tail.chars().count();
    let is_partial = (count < OPEN.chars().count() && OPEN.starts_with(tail))
        || (count < CLOSE.chars().count() && CLOSE.starts_with(tail));
    if is_partial {
        bracket
    } else {
        buffer.len()
    }
}

/// 标记里的那一串是不是**固定枚举**里的一员（压掉空白后逐字比）。
pub fn normalize_phase(raw: &str) -> Option<String> {
    let collapsed: String = raw.split_whitespace().collect::<Vec<_>>().join("");
    PHASES.iter().find(|p| **p == collapsed).map(|p| (*p).to_string())
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
        assert_eq!(text, "我先看看需求。读一下工作流。", "标记本身不该出现在正文里");
        assert_eq!(phases, vec!["正在分析请求".to_string(), "正在读文件".to_string()]);
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
        let (text, phases) =
            stripper.push("[步骤]正在分析请求[/步骤][步骤]正在写文件[/步骤]写完了");
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
}
