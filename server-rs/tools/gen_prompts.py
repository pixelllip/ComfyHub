"""⛔ **本脚本已停用：它拒绝写 `src/nsfw_prompt.rs`。**

原本的用途：从 Kotlin 源码里**机械提取**提示词正文，生成 Rust 常量（P4：run-kit 搬到 Rust）。
当初的理由成立 —— 这几段正文加起来 5 万多字符，手抄一个字的差错就是"两端规则不一致"（红线 R13），
而且**看不出来**。判据是"同一个库、同一份 storage 上，Rust 与 Kotlin 的 `/api/ai/run-kit`
canon 后零差异"。

**但那个前提已经不成立了（2026-09-27 实测）**：两端真的分叉，Kotlin 不再是唯一真源。
跑一次生成 = 一次回归，所以 `main()` 现在只打印理由并**非零退出**，一个字节都不写。
正文的真源是 `server-rs/src/nsfw_prompt.rs`（见它的文件头）；要改正文就改那里，
并 bump `PROMPT_VERSION`（红线 R12）。

助手函数（`trim_indent` / `raw_block` / `extract_raw_string` …）留着，作为
"这份正文当初是怎么从 Kotlin 抠出来的"的**参考实现**（当年的 `trimIndent()` 语义、
插值换哨兵的写法都在这里），但要理解成**只读的历史**，不要再接回去生成。

用法：`python server-rs/tools/gen_prompts.py` —— 预期结果就是**拒绝 + 非零退出**。
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
# ⚠ 这两条路径**已经不存在**（Kotlin 于 P4 C-6 退役时搬到 `legacy/server/`）。
# 刻意不改：改了只会让人以为"跑一下就好"，而真正的问题不是路径，是两端内容已经分叉。
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


# ⚠ 这是**旧的**输出骨架（`main()` 已经不用它了）—— 留作历史参考。
# 里面的文件头写着"自动生成，别手改"，那句话**已经作废**：真源就是 `src/nsfw_prompt.rs` 本身。
# 别照抄这段模板去覆盖任何东西（它会把 `PROMPT_VERSION` 钉回生成时的版本）。
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


REFUSAL = """\
⛔ 拒绝生成：{out}
   这份文件现在是**系统提示正文的真源**，从 Kotlin 生成一次就是一次回归（2026-09-27 实测）。

① 路径本身已经失效：Kotlin 树在 P4 C-6 退役时搬到了 `legacy/server/`，
   本脚本读的 `{nsfw}` 不存在（就算改到 legacy/，下面两条也照样成立）。

② `PROMPT_VERSION` 会**从 v33 退回 v31**（它从 legacy 的 `HarnessRunner.kt`
   读版本号），并且模板会把"Rust 为什么从 v32 走到 v33"那段注释**整段删掉** ——
   后人再也看不到"跳过 v31 / 第 19 条只存在于 Rust"这段缘由。

③ 第 19 条（图像编辑）那批 **2026-09-27 的实测结论只在 Rust 侧**，
   Kotlin 的 `NsfwPrompt.kt` 里一个字都没有（已 grep 确认：`mattingModels` /
   `先想能不能不抠` / `别默默降级` / `krea2 不适合` 全部 absent）。生成一次，
   下面这些结论就**整段消失**，模型会退回错误行为：
     · 贴人物图首选 AI 软遮罩（先查 `mattingModels.usable`）；
     · 缺模型要指引用户下载，别硬跑、也别默默降级成颜色抠图（实测掉 18.4% 前景）；
     · `comfyhub_inpaint_krea2.json` **不适合做编辑**（只留给文生图）。

（第 20 条配方规则曾经也是 Kotlin 独有；P4 第 69 步已按原文补进 Rust 的 v33，
 所以它不再构成差异 —— 但 ② ③ 已经足够让这个脚本不能写。）

要改正文：直接改 `server-rs/src/nsfw_prompt.rs`，并 bump `PROMPT_VERSION`（红线 R12）。
本脚本的提取助手只作**历史参考**（当年怎么从 Kotlin 抠出来、`trimIndent()` 什么语义）。
"""


def main():
    print(
        REFUSAL.format(out=OUT.relative_to(ROOT), nsfw=NSFW_KT.relative_to(ROOT)),
        file=sys.stderr,
    )
    print("（退出码 2：一个字节都没写。）", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
