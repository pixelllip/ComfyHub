//! 会话标题的**自动总结**（Kotlin `ConversationTitle.kt` 的逐条移植）。
//!
//! 为什么是"让模型在正文最前面带一行标题"，而不是再发一次上游请求去总结：
//! 第一条提问本来就要走一次上游，多问一次就是多花一次钱、多等一次网络。
//! 所以第一轮附一句很短的要求，让模型先输出 `[标题]…[/标题]`，
//! 后端在流式过程中把它摘掉、写进会话，**用户看不到这行标记**。
//!
//! 这个类就是"摘标记"的状态机，**纯逻辑、可单测**。两条铁律：
//!  1. **绝不吞内容** —— 模型不按格式来时，说过的话一个字都不能少；
//!  2. **绝不重复内容** —— 寒暄可能分好几片到达，已放行的部分不能再放一遍。
//!
//! 四个状态（[`TitleStripper::scan_and_strip`] 是"扫描 + 摘标记"的循环）：
//!
//! | 状态 | 含义 |
//! | --- | --- |
//! | `Start` | 还没看见 `[标题]` → 先压着（最多 [`MAX_START_LOOKAHEAD`] 个字符），免得把切成两片的 `[标`+`题]` 漏半个给用户 |
//! | `Stream` | 已经在放行寒暄了；尾部最多留标记长度那么长的尾巴，防止标记被切开时漏出去 |
//! | `InTitle` | 见到 `[标题]`，正在等 `[/标题]`（标题本身不放进正文） |
//! | `Passthrough` | 已经摘到标题：正文正常放行，但**继续盯着**完整标记对 —— 模型在后续轮次里可能再吐一遍（回声），那同样是标记、不是正文 |
//!
//! ⚠ 回声**只摘标记、不改已定的会话名**（[`TitleStripper::title`] 只有第一次摘到的那个）。

/// 标题标记：模型被告知只输出这一个格式。
pub const OPEN: &str = "[标题]";
pub const CLOSE: &str = "[/标题]";
/// 标题长度上限。
pub const MAX_TITLE_CHARS: usize = 60;
/// `Start` 状态最多压多少字符（足够覆盖"先啰嗦两句再给标题"的常见偏差，
/// 又不会让第一句话迟迟不出现在界面上）。
pub const MAX_START_LOOKAHEAD: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Start,
    Stream,
    InTitle,
    Passthrough,
}

/// 摘标题的状态机。
pub struct TitleStripper {
    enabled: bool,
    buffer: String,
    state: State,
    /// 刚摘掉标记：接下来放出去的那一小段要先剪掉前导空白
    /// （模型被要求"标题之后空一行"，不剪的话气泡顶部会空一块 —— 真机实测过）。
    trim_leading_blank: bool,
    title: Option<String>,
}

impl TitleStripper {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            buffer: String::new(),
            state: State::Start,
            trim_leading_blank: false,
            title: None,
        }
    }

    /// 摘出来的标题（还没摘到时为 `None`）。
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// 标记已经处理完。⚠ **不等于"后面不会再有标记"**：回声照样会被摘掉。
    pub fn finished(&self) -> bool {
        !self.enabled || self.state == State::Passthrough
    }

    /// 喂一片增量，返回**应当进入正文**的文本（可能为空串）。
    ///
    /// 调用方只需把返回值原样发给界面 / 追加到落库文本，不要另外拼接缓冲。
    pub fn push(&mut self, chunk: &str) -> String {
        if !self.enabled || chunk.is_empty() {
            return chunk.to_string();
        }
        self.buffer.push_str(chunk);
        if self.state == State::Start {
            // 还没看见 `[标题]`：先按住（最多 MAX_START_LOOKAHEAD）
            if !self.buffer.contains(OPEN) && self.buffer.chars().count() <= MAX_START_LOOKAHEAD {
                return String::new();
            }
            // 压得太久说明模型根本没打算给标题：放行（尾部仍留可能是标记开头的那几个字符）
            self.state = State::Stream;
        }
        self.scan_and_strip()
    }

    /// 流结束时收尾：没闭合的缓冲一律当正文放出去（**宁可多显示，也不吞内容**）。
    pub fn flush(&mut self) -> String {
        if self.buffer.is_empty() {
            return String::new();
        }
        self.state = State::Passthrough;
        self.take_buffer()
    }

    // -----------------------------------------------------------------------

    fn scan_and_strip(&mut self) -> String {
        let mut out = String::new();
        loop {
            if self.state == State::InTitle {
                match self.buffer.find(CLOSE) {
                    None => {
                        // 标题本身长得离谱 → 认输：`[标题]` 之后那一段当正文放行
                        if self.buffer.chars().count() > MAX_TITLE_CHARS + 40 {
                            self.state = State::Passthrough;
                            let text = self.take_buffer();
                            out.push_str(&self.clean_leading(&text));
                        }
                        return out;
                    }
                    Some(close_at) => {
                        let raw = self.buffer[..close_at].to_string();
                        if self.title.is_none() {
                            self.title = clean_title(&raw);
                            // 标题之后紧跟的那一个空行剪掉（只对第一次这样做；回声前后一概不碰）
                            self.trim_leading_blank = true;
                        }
                        self.buffer.drain(..close_at + CLOSE.len());
                        self.state = State::Passthrough;
                        continue;
                    }
                }
            }

            match self.buffer.find(OPEN) {
                None => {
                    // 尾部可能藏着切了一半的标记（`[` / `[标` / `[/标`），留到下一片再说
                    let keep = last_partial_marker_start(&self.buffer);
                    let text = self.drop_before(keep);
                    out.push_str(&self.clean_leading(&text));
                    return out;
                }
                Some(open_at) => {
                    // 标记之前的正文（寒暄）是模型真的说了的话，原样放行
                    let text = self.drop_before(open_at);
                    out.push_str(&self.clean_leading(&text));
                    self.buffer.drain(..OPEN.len());
                    self.state = State::InTitle;
                }
            }
        }
    }

    fn take_buffer(&mut self) -> String {
        std::mem::take(&mut self.buffer)
    }

    /// 放行 `[0, end)`，把它们从缓冲里去掉（`end` 之前的都确定不是标记的一部分）。
    fn drop_before(&mut self, end: usize) -> String {
        if end == 0 {
            return String::new();
        }
        let end = end.min(self.buffer.len());
        let out = self.buffer[..end].to_string();
        self.buffer.drain(..end);
        out
    }

    /// 标题标记之后的第一段内容：剪掉前导空白（见 `trim_leading_blank`）。
    fn clean_leading(&mut self, text: &str) -> String {
        if !self.trim_leading_blank || text.is_empty() {
            return text.to_string();
        }
        let trimmed = text.trim_start();
        if !trimmed.is_empty() {
            self.trim_leading_blank = false;
        }
        trimmed.to_string()
    }
}

/// 缓冲末尾那段"可能还没收全的标记"从哪个下标开始（`[标` / `[/标` 的前缀）；
/// 其余一律可以立刻放行 —— 否则模型一开口就被按住，流式看起来像卡住。
fn last_partial_marker_start(buffer: &str) -> usize {
    let Some(bracket) = buffer.rfind('[') else { return buffer.len() };
    let tail = &buffer[bracket..];
    let is_partial = (tail.chars().count() < OPEN.chars().count() && OPEN.starts_with(tail))
        || (tail.chars().count() < CLOSE.chars().count() && CLOSE.starts_with(tail));
    if is_partial {
        bracket
    } else {
        buffer.len()
    }
}

/// 标题文本的清洗：压空白、去掉模型爱加的前缀与包裹（`标题：xxx`、`《xxx》`、`"xxx"`…）。
fn clean_title(raw: &str) -> Option<String> {
    let collapsed: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let trim_chars = ['：', ':', '"', '\'', '“', '”', '‘', '’', '《', '》', '【', '】', '「', '」', '[', ']', '#', '*'];
    let mut text = collapsed.trim().trim_matches(|c| trim_chars.contains(&c)).to_string();
    if let Some(rest) = text.strip_prefix("标题") {
        text = rest.trim_start_matches(['：', ':', ' ', '\t']).to_string();
    }
    text = text.trim_matches(|c| trim_chars.contains(&c)).to_string();
    if text.chars().count() > MAX_TITLE_CHARS {
        text = text.chars().take(MAX_TITLE_CHARS).collect();
    }
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
