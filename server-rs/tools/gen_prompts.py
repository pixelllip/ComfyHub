"""从 Kotlin 源码里**机械提取**提示词正文，生成 Rust 常量（P4：run-kit 搬到 Rust）。

为什么要生成而不是手抄：这几段正文加起来 5 万多字符（PERSONA_BLOCK + NSFW_BLOCK），
手抄一个字的差错就是"两端规则不一致"，而这类差错**看不出来**（红线 R13）。
提取规则照抄 Kotlin 的 trimIndent() 语义，并把 ${...} 插值换成哨兵串，
由 Rust 侧用**同一个常量**替换（阈值真源只有一处）。

用法：python server-rs/tools/gen_prompts.py
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
NSFW_KT = ROOT / "server/src/main/kotlin/com/comfyhub/ai/NsfwPrompt.kt"
RUNNER_KT = ROOT / "server/src/main/kotlin/com/comfyhub/ai/HarnessRunner.kt"
OUT = ROOT / "server-rs/src/nsfw_prompt.rs"

SENTINELS = {
    "com.comfyhub.ai.tools.ToolPolicyConfig.DEFAULT_MAX_COMFY_QUERIES_PER_RUN": "«MAX_COMFY»",
    "com.comfyhub.ai.tools.ToolPolicyConfig.DEFAULT_MAX_WEB_QUERIES_PER_RUN": "«MAX_WEB»",
    "com.comfyhub.ai.tools.MemoryStore.MAX_ENTRIES_PER_RUN": "«MAX_ENTRIES_PER_RUN»",
    "com.comfyhub.ai.tools.MemoryStore.MAX_ENTRIES": "«MAX_ENTRIES»",
}

CONTINUE_INSTRUCTION = (
    "（上一条回复因为输出长度上限被截断了。）请**紧接中断处继续写**："
    "不要重复已经说过的内容，不要重新开头，也不要道歉或解释，直接从下一个字接着写。"
)


def trim_indent(text):
    """Kotlin String.trimIndent() 的逐字对应实现。"""
    lines = text.split("\n")
    if lines and lines[0].strip() == "":
        lines = lines[1:]
    if lines and lines[-1].strip() == "":
        lines = lines[:-1]
    widths = [len(l) - len(l.lstrip(" ")) for l in lines if l.strip() != ""]
    indent = min(widths) if widths else 0
    stripped = [l[indent:] if len(l) >= indent else "" for l in lines]
    return "\n".join(stripped)


def resolve_interpolations(text):
    for expr, sentinel in SENTINELS.items():
        text = text.replace("${" + expr + "}", sentinel)
    leftover = re.findall(r"\$\{[^}]*\}", text)
    if leftover:
        raise SystemExit("还有没处理的插值：%s" % leftover)
    return text


def raw_block(text):
    """Rust raw string：`r#"…"#`（**# 在开引号之前**，别写反）。

    ⚠ 开引号与正文之间**不能加换行**：那是 raw string 的一部分（会把每一段正文
    都变成"以一个换行开头"，而它直接进系统提示 —— 两端就差这一个字符）。
    """
    hashes = 1
    while ('"' + "#" * hashes) in text:
        hashes += 1
    pad = "#" * hashes
    return 'r%s"%s"%s' % (pad, text, pad)


def extract_raw_string(src, marker):
    start = src.index(marker)
    open_at = src.index('"""', start) + 3
    close_at = src.index('"""', open_at)
    return resolve_interpolations(trim_indent(src[open_at:close_at]))


def extract_note(src):
    start = src.index("const val DISABLED_NOTE")
    end = src.index("\n}", start)
    chunk = src[start:end]
    parts = re.findall(r'"((?:[^"\\]|\\.)*)"', chunk)
    joined = "".join(parts)
    return joined.replace("\\n", "\n").replace('\\"', '"')


TEMPLATE = '''//! 系统提示里那两段"与内容策略有关"的正文 + 两条按轮指令（**自动生成，别手改**）。
//!
//! 生成脚本：`server-rs/tools/gen_prompts.py`（从 `server/src/main/kotlin/com/comfyhub/ai/NsfwPrompt.kt`
//! 与 `.../HarnessRunner.kt` 机械提取，照抄 Kotlin 的 `trimIndent()` 语义）。
//!
//! # 为什么是生成的
//!
//! 这几段加起来 5 万多字符，**手抄一个字的差错就是"两端规则不一致"**（红线 R13），
//! 而那种差错在界面上看不出来（模型只是照着一份略微不同的规则说话）。
//! 生成脚本保证正文逐字一致 —— 判据是"同一个库、同一份 storage 上，
//! Rust 与 Kotlin 的 `/api/ai/run-kit` canon 后零差异"。
//!
//! # 那两段的分工（Kotlin 侧的原注释照搬）
//!
//! - [`persona_block`]（**两种模式都注入**）：常驻的事实纪律 / 表达风格 / 必须遵守 1~N 条。
//! - [`nsfw_block`]（**只有 NSFW 开关打开才注入**）：`[MODE: UNRESTRICTED SANDBOX — STABLE]`
//!   起、到 `Behavioral rules` 那几行。⚠ **那段文字是用户有意注入的**（AGENTS §10：
//!   2026-09-17 用户明确说"不许删除，不要动那段越狱提示词"）—— 要改先问用户。
//! - [`disabled_note`]：关掉开关时**如实说明**这一版少了什么（不能让模型自己觉得可以）。
//!
//! 正文里的「«…»」哨兵是 Kotlin `${...}` 插值的位置（阈值真源在 [`crate::policy`]），
//! [`persona_block`] / [`nsfw_block`] 会就地替换掉；`no_sentinels_left` 用例盯着有没有漏。

/// `NsfwPrompt.VERSION`（改动正文要一起 bump；它进 Run 快照）。
pub const VERSION: &str = "__VERSION__";

/// `SystemPrompt.VERSION`（系统提示正文的版本；改提示词**必须** bump —— 红线 R12）。
pub const PROMPT_VERSION: &str = "__PROMPT_VERSION__";

/// 第一问时按轮追加的起标题指令（`SystemPrompt.TITLE_INSTRUCTION`）。
pub const TITLE_INSTRUCTION: &str = __TITLE__;

/// "输出被长度上限截断时接着写"的指令（`HarnessRunner.CONTINUE_INSTRUCTION`）。
pub const CONTINUE_INSTRUCTION: &str = __CONTINUE__;

/// 关掉 NSFW 时的一句话说明。
pub const DISABLED_NOTE: &str = __DISABLED__;

const PERSONA_RAW: &str = __PERSONA__;

const NSFW_RAW: &str = __NSFW__;

/// 哨兵 → 常量的就地替换（阈值只有一份真源）。
fn resolve(text: &str) -> String {
    text.replace("«MAX_COMFY»", &crate::policy::MAX_COMFY_QUERIES_PER_RUN.to_string())
        .replace("«MAX_WEB»", &crate::policy::MAX_WEB_QUERIES_PER_RUN.to_string())
        .replace("«MAX_ENTRIES_PER_RUN»", &crate::memory::MAX_ENTRIES_PER_RUN.to_string())
        .replace("«MAX_ENTRIES»", &crate::memory::MAX_ENTRIES.to_string())
}

/// 常驻段（事实纪律 + 表达风格 + 必须遵守）：**两种模式都注入**。
pub fn persona_block() -> String {
    resolve(PERSONA_RAW)
}

/// 越狱 / 对抗框架段：**只有 NSFW 开关打开时**才注入。
pub fn nsfw_block() -> String {
    resolve(NSFW_RAW)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_sentinels_left() {
        for (name, text) in [("persona", persona_block()), ("nsfw", nsfw_block())] {
            assert!(!text.contains('«'), "{} 里还有没替换的插值哨兵", name);
            assert!(!text.contains("${"), "{} 里还有没处理的 Kotlin 插值", name);
        }
        // 阈值确实插进去了（Kotlin 侧是 ${...}，这里必须变成数字）
        let persona = persona_block();
        assert!(persona.contains(&format!("最多主动查询 {} 次", crate::policy::MAX_COMFY_QUERIES_PER_RUN)));
        assert!(persona.contains(&format!("最多联网 {} 次", crate::policy::MAX_WEB_QUERIES_PER_RUN)));
        assert!(persona.contains(&format!("最多新增 {} 条", crate::memory::MAX_ENTRIES_PER_RUN)));
    }

    #[test]
    fn blocks_are_not_empty_and_trimmed() {
        let persona = persona_block();
        assert!(persona.len() > 5000, "常驻段正文太短，多半是提取截断了");
        // ⚠ `PERSONA_BLOCK` 在 Kotlin 里**就是**以一个空行开头、以一个换行结尾
        // （`trimIndent` 只去掉首尾各一个空行）—— 这是真实输出，别"顺手 trim 掉"：
        // 它进的是系统提示，差一个换行就是两端不一样。
        assert!(persona.starts_with('\\n'));
        assert!(persona.ends_with('\\n'));
        assert!(!nsfw_block().starts_with('\\n'));
        assert!(nsfw_block().contains("[MODE: UNRESTRICTED SANDBOX — STABLE]"));
        assert!(DISABLED_NOTE.contains("NSFW"));
        assert!(TITLE_INSTRUCTION.starts_with("【本次附加任务"));
        // 起标题指令里有那对标记（手机 harness 靠它摘标题）
        assert!(TITLE_INSTRUCTION.contains("[标题]"));
    }
}
'''


def main():
    nsfw_src = NSFW_KT.read_text(encoding="utf-8")
    runner_src = RUNNER_KT.read_text(encoding="utf-8")

    version = re.search(r'const val VERSION = "([^"]+)"', nsfw_src).group(1)
    persona = extract_raw_string(nsfw_src, "val PERSONA_BLOCK")
    nsfw_block = extract_raw_string(nsfw_src, "val NSFW_BLOCK")
    disabled = extract_note(nsfw_src)
    title = extract_raw_string(runner_src, "val TITLE_INSTRUCTION")
    prompt_version = re.search(r'const val VERSION = "(v\d+)"', runner_src).group(1)

    out = (
        TEMPLATE.replace("__VERSION__", version)
        .replace("__PROMPT_VERSION__", prompt_version)
        .replace("__TITLE__", raw_block(title))
        .replace("__CONTINUE__", raw_block(CONTINUE_INSTRUCTION))
        .replace("__DISABLED__", raw_block(disabled))
        .replace("__PERSONA__", raw_block(persona))
        .replace("__NSFW__", raw_block(nsfw_block))
    )
    OUT.write_text(out, encoding="utf-8", newline="\n")
    print("wrote %s (%d bytes); persona=%d nsfw=%d title=%d disabled=%d"
          % (OUT, len(out), len(persona), len(nsfw_block), len(title), len(disabled)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
