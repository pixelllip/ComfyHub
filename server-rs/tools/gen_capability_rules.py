"""从 Kotlin `ModelCapabilityCatalog.kt` 生成 Rust 的内置能力规则表。

为什么要生成而不是手抄：那张表有 **110 条**规则（模式 / 模态 / 工具 / 推理 / 思考档位 /
思考方言），手抄必然漏几条 —— 而漏掉的表现是"某个模型在 Rust 后端上图片输入没勾上"，
界面上完全看不出来（只是导入时少预填一个选项）。

用法（在仓库根或任意目录都行，路径相对本文件解析）：:

    python server-rs/tools/gen_capability_rules.py            # 写 src/capability_rules.rs
    python server-rs/tools/gen_capability_rules.py --check     # 只校验，不写（CI/回归用）

解析不出来的一律**直接报错**（绝不悄悄产出一份残缺的表）—— 与 `gen-builtin-catalog.ps1` 同一条纪律。
"""
import argparse
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SERVER_RS = HERE.parent
ROOT = SERVER_RS.parent
KOTLIN = ROOT / "server" / "src" / "main" / "kotlin" / "com" / "comfyhub" / "ai" / "ModelCapabilityCatalog.kt"
OUT = SERVER_RS / "src" / "capability_rules.rs"


def fail(message):
    print(f"生成失败：{message}", file=sys.stderr)
    sys.exit(1)


def parse_efforts(text):
    """`EFFORT_*` 定义 → {名字: [(等级, 表达), …]}。

    支持两种写法：`mapOf(a to "b", …)` 与 `OTHER + ("max" to "max")`。
    """
    efforts = {}
    for match in re.finditer(r"private val (EFFORT_[A-Z_]+)\s*=\s*(.+?)(?=\n\s*(?:private val|/\*\*|/\*|//|\}\s*$))", text, re.S):
        name, body = match.group(1), match.group(2)
        body = body.strip()
        base = None
        extra = []
        # 先看有没有 `X + (k to v)` 这种组合
        plus = re.match(r"^(EFFORT_[A-Z_]+)\s*\+\s*\((.*)\)\s*$", body, re.S)
        if plus:
            base = plus.group(1)
            extra = re.findall(r'"([a-z]+)"\s*to\s*"([^"]*)"', plus.group(2))
        pairs = re.findall(r'"([a-z]+)"\s*to\s*"([^"]*)"', body)
        if not pairs:
            fail(f"{name} 里没解析出任何 `k to v`：{body!r}")
        efforts[name] = (base, pairs, extra)
    return efforts


def resolve_efforts(efforts):
    """把组合展开成最终的有序表（`off` 在前，与 Kotlin 里的顺序一致）。"""
    resolved = {}

    def build(name, seen=None):
        if name in resolved:
            return resolved[name]
        seen = seen or set()
        if name in seen:
            fail(f"{name} 的定义成环了")
        seen.add(name)
        if name not in efforts:
            fail(f"引用了不存在的档位表 {name}")
        base, pairs, extra = efforts[name]
        out = []
        if base:
            out.extend(build(base, seen))
        out.extend((k, v) for k, v in pairs if (k, v) not in out)
        out.extend((k, v) for k, v in extra if (k, v) not in out)
        resolved[name] = out
        return out

    for name in efforts:
        build(name)
    return {name: build(name) for name in efforts}


def parse_rules(text):
    start = text.find("private val rules")
    if start < 0:
        fail("找不到 `private val rules`")
    body_start = text.find("buildList {", start)
    if body_start < 0:
        fail("找不到 `buildList {`")
    # 取到与之匹配的收尾 `}`（按花括号计数）
    depth = 0
    i = body_start + len("buildList")  # 指向 `{`
    end = -1
    while i < len(text):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                end = i
                break
        i += 1
    if end < 0:
        fail("buildList 的花括号不配对")
    body = text[body_start:end]

    rules = []
    # 只认 `rule("…")`（第一个实参是字符串字面量）—— `buildList` 里那个本地
    # `fun rule(pattern: String, …)` 声明必须被跳过（它也是 `rule(` 开头）
    for match in re.finditer(r'\brule\((?=")', body):
        # 括号平衡地把整个调用取出来
        depth = 0
        j = match.end() - 1
        while j < len(body):
            if body[j] == "(":
                depth += 1
            elif body[j] == ")":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        if j >= len(body):
            fail("rule( 的括号不配对")
        call = " ".join(body[match.end():j].split())
        parsed = parse_rule(call)
        if parsed:
            rules.append(parsed)
    if not rules:
        fail("一条规则都没解析出来")
    return rules


def parse_rule(call):
    """一条 `rule(...)` 的参数 → 元组。只认表里真实用到的三种写法。"""
    m = re.match(
        r'^"([^"]+)"\s*,\s*(TEXT_ONLY|TEXT_IMAGE)\s*,\s*tools\s*=\s*(true|false)'
        r'(?:\s*,\s*reasoning\s*=\s*(true|false))?'
        r'(?:\s*,\s*efforts\s*=\s*(EFFORT_[A-Z_]+))?'
        r'(?:\s*,\s*format\s*=\s*"([^"]+)")?\s*$',
        call,
    )
    if not m:
        fail(f"认不出来的 rule 写法：{call!r}")
    pattern, modalities, tools, reasoning, efforts, fmt = m.groups()
    return {
        "pattern": pattern,
        "modalities": modalities,
        "tools": tools == "true",
        "reasoning": reasoning == "true",
        "efforts": efforts,
        "format": fmt,
    }


HEADER = '''//! 内置能力规则表（`ModelCapabilityCatalog.rules` 的 Rust 端落点）。
//!
//! **自动生成，别手改**：`python server-rs/tools/gen_capability_rules.py`
//! （源 = `server/src/main/kotlin/com/comfyhub/ai/ModelCapabilityCatalog.kt`，
//! `--check` 可以只校验不写）。理由同 `mobile_tools.rs` / `nsfw_prompt.rs`：
//! 这张表有 **{count} 条**，手抄必然漏几条 —— 而漏掉的表现是"某个模型在 Rust 后端上
//! 图片输入没勾上"，界面上完全看不出来（只是导入时少预填一个选项）。
//!
//! 用法只有一个：`capability_catalog::rules()`（`builtinRules` 的条数也从它来）。
//! 匹配是**包含匹配**、**先命中先用**（宽前缀规则排后面，免得 `gpt-4` 抢走 `gpt-4o`），
//! 所以**顺序本身就是语义**，生成时不许排序。
'''


def emit(rules, efforts):
    used = sorted({r["efforts"] for r in rules if r["efforts"]})
    lines = [HEADER.format(count=len(rules)), "", "use crate::capability_catalog::RawRule;", ""]
    lines.append('const TEXT_ONLY: &[&str] = &["text"];')
    lines.append('const TEXT_IMAGE: &[&str] = &["text", "image"];')
    lines.append("")
    for name in used:
        pairs = ", ".join(f'("{k}", "{v}")' for k, v in efforts[name])
        lines.append(f"const {name}: &[(&str, &str)] = &[{pairs}];")
    lines.append("")
    lines.append("/// 内置规则（**顺序即语义**，别排序）。")
    lines.append("pub const RAW_RULES: &[RawRule] = &[")
    for r in rules:
        efforts_arg = f"Some({r['efforts']})" if r["efforts"] else "None"
        fmt = f'Some("{r["format"]}")' if r["format"] else "None"
        tools = "true" if r["tools"] else "false"
        reasoning = "true" if r["reasoning"] else "false"
        lines.append(
            "    RawRule {"
            f' pattern: "{r["pattern"]}", modalities: {r["modalities"]}, tools: {tools},'
            f' reasoning: {reasoning}, efforts: {efforts_arg}, format: {fmt} }},'
        )
    lines.append("];")
    lines.append("")
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="只校验生成结果与磁盘上那份一致")
    args = ap.parse_args()

    if not KOTLIN.is_file():
        fail(f"找不到源文件 {KOTLIN}")
    text = KOTLIN.read_text(encoding="utf-8")
    efforts = resolve_efforts(parse_efforts(text))
    rules = parse_rules(text)
    generated = emit(rules, efforts)

    if args.check:
        current = OUT.read_text(encoding="utf-8") if OUT.is_file() else ""
        if current != generated:
            fail(f"{OUT.name} 与 Kotlin 源不一致（重跑不带 --check 的生成）")
        print(f"{OUT.name} 与源一致（{len(rules)} 条规则）")
        return
    OUT.write_text(generated, encoding="utf-8", newline="\n")
    print(f"已写出 {OUT}（{len(rules)} 条规则、{len(efforts)} 张档位表）")


if __name__ == "__main__":
    main()
