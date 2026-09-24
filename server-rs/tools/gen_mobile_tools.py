"""从 Kotlin 端点的真实响应里提取手机工具清单（name / description / inputSchema），
生成 `server-rs/src/mobile_tools.rs`。

为什么要生成：`inputSchema` 是懒构建的 JSON-Schema DSL 拼出来的，手抄几百行 schema
必然漏键；而**少一个 schema 键 = 模型给不出正确参数**，且没有任何测试会变红。
判据同 `gen_prompts.py`：同一份库上 Rust 与 Kotlin 的 `/api/ai/run-kit` canon 后零差异。

前置：Kotlin 后端在 8090 上跑着（`.run/runkit/baseline/kotlin-run-kit.json` 是它的响应）。
用法：python server-rs/tools/gen_mobile_tools.py
"""

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BASELINE = ROOT / ".run/runkit/baseline/kotlin-run-kit.json"
OUT = ROOT / "server-rs/src/mobile_tools.rs"

# ToolRegistry.kt 里的出厂权限档与"会不会写数据"（prompt 里那三个标记用它）：
# 见 `python .run/runkit/tool_flags.py` 的输出。改了 Kotlin 那边要同步改这里。
FLAGS = {
    "ask_user_question": ("ALLOW", False),
    "civitai_search": ("ASK", False),
    "list_skills": ("ALLOW", False),
    "load_skill": ("ALLOW", False),
    "remember": ("ALLOW", True),
    "web_fetch": ("ASK", False),
    "web_search": ("ASK", False),
}


def raw_block(text):
    """Rust raw string：`r#"…"#`（**# 在开引号之前**，别写反）。

    ⚠ 开引号与正文之间**不能加换行**：那是 raw string 的一部分（会让每个 schema
    都以换行开头 → 下发的 JSON Schema 非法 → 手机侧工具参数全错）。
    """
    hashes = 1
    while ('"' + "#" * hashes) in text:
        hashes += 1
    pad = "#" * hashes
    return 'r%s"%s"%s' % (pad, text, pad)


TEMPLATE = '''//! 手机可执行工具清单（ToolScope = `MOBILE` / `BOTH`，🔒 `docs/tool-scope-contract.md`）。
//!
//! **自动生成，别手改**：`python server-rs/tools/gen_mobile_tools.py`（从 Kotlin `ToolRegistry`
//! 经 `/api/ai/run-kit` 吐出来的真实 schema 提取）。理由同 `nsfw_prompt.rs`：
//! 这些 `inputSchema` 是懒构建的 JSON-Schema，手抄几百行必然漏键 ——
//! 而**少一个 schema 键 = 模型给不出正确参数**，界面上看不出来。
//!
//! # 它同时是两件事的真源
//!
//! 1. `GET /api/ai/run-kit` 的 `tools[]`（手机 harness 的"能调什么"，
//!    红线 R12：工具清单只有一份真源 —— 桌面 `ToolRegistry`，这里是它的 Rust 端落点）；
//! 2. 系统提示里"可调用的工具"那一节的正文（名字 + 权限标记 + 描述前 300 字）。
//!
//! ⚠ 这里**只有** `scope ∈ {MOBILE, BOTH}` 的 7 个。桌面专属的（`comfy_*` / `read_file` /
//! `register_skill` …）**不许出现在手机清单里**（总纲 §3.5：不是安全边界，是
//! "别让模型看见它调不到的工具"）。R11：手机侧不许复用 `comfy_submit` 这个名字。

/// 一个手机可执行工具（字段与 🔒 `docs/run-kit-contract.md` §2 的 `tools[]` 一一对应）。
pub struct MobileToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema 原文（原样进 `tools[].inputSchema`）。
    pub input_schema: &'static str,
    /// 线上一律大写（`MOBILE` / `BOTH`）—— 手机侧按它判"这个我能不能执行"。
    pub scope: &'static str,
    /// 出厂权限档（`ALLOW` / `ASK` / `DENY`）；用户在设置里可以逐工具覆盖。
    pub access: &'static str,
    /// 会不会写数据（prompt 里"（会写数据）/（只读）"那个标记）。
    pub mutating: bool,
}

/// ⚠ **按 name 升序**：Kotlin `RunKit.kitParts` 里是 `.sortedBy { it.name }`，
/// 而名字列表还进了 `kitVersion` 的内容签名 —— 顺序变了版本号会跟着跳。
pub const MOBILE_TOOLS: &[MobileToolSpec] = &[
__TOOLS__];

/// 不可信数据横幅的**模板**（R7：两端不许各写一份）。
/// 真源是 Kotlin `ToolRegistry.UNTRUSTED_BANNER_TEMPLATE`，与 `UNTRUSTED_BANNER_QUERY_LINE`
/// 拼起来就是下发的那一整条 `untrustedBanner`。
pub const UNTRUSTED_BANNER_TEMPLATE: &str = __BANNER__;

/// 横幅第二行（**仅在有查询词时**带）。
pub const UNTRUSTED_BANNER_QUERY_LINE: &str = __QUERY_LINE__;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_sorted_and_mobile_only() {
        let mut names: Vec<&str> = MOBILE_TOOLS.iter().map(|t| t.name).collect();
        let sorted = {
            let mut n = names.clone();
            n.sort();
            n
        };
        assert_eq!(names, sorted, "工具必须按 name 升序（顺序进了 kitVersion 签名）");
        names.dedup();
        assert_eq!(names.len(), MOBILE_TOOLS.len(), "工具名不许重复");
        for tool in MOBILE_TOOLS {
            assert!(
                tool.scope == "MOBILE" || tool.scope == "BOTH",
                "{} 的 scope 不是手机可执行档",
                tool.name
            );
            assert!(!tool.name.starts_with("comfy_submit"), "R11：手机侧不许复用提交工具名");
        }
    }

    #[test]
    fn every_schema_is_parseable_json() {
        for tool in MOBILE_TOOLS {
            let value: serde_json::Value =
                serde_json::from_str(tool.input_schema).unwrap_or_else(|e| panic!("{} 的 schema 不是 JSON: {e}", tool.name));
            assert_eq!(value["type"], "object", "{} 的 schema 顶层必须是 object", tool.name);
        }
    }
}
'''


def main():
    data = json.loads(BASELINE.read_text(encoding="utf-8"))
    tools = sorted(data["tools"], key=lambda t: t["name"])

    blocks = []
    for tool in tools:
        name = tool["name"]
        access, mutating = FLAGS[name]
        blocks.append(
            "    MobileToolSpec {\n"
            '        name: "%s",\n'
            "        description: %s,\n"
            "        input_schema: %s,\n"
            '        scope: "%s",\n'
            '        access: "%s",\n'
            "        mutating: %s,\n"
            "    },\n"
            % (
                name,
                raw_block(tool["description"]),
                raw_block(json.dumps(tool["inputSchema"], ensure_ascii=False, separators=(",", ":"))),
                tool["scope"],
                access,
                "true" if mutating else "false",
            )
        )

    banner = data["untrustedBanner"]
    marker = "\n查询："
    assert marker in banner, "横幅形状变了：%r" % banner
    template, query_line = banner.split(marker, 1)
    query_line = marker + query_line

    out = (
        TEMPLATE.replace("__TOOLS__", "".join(blocks))
        .replace("__BANNER__", raw_block(template))
        .replace("__QUERY_LINE__", raw_block(query_line))
    )
    OUT.write_text(out, encoding="utf-8", newline="\n")
    print("wrote %s (%d bytes); tools=%s" % (OUT, len(out), [t["name"] for t in tools]))

    # 顺带把 models[] 的键集合打出来（写 DTO 时按它对齐）
    keys = set()
    for m in data["models"]:
        keys |= set(m.keys())
    print("models 出现过的键:", sorted(keys))
    missing = {k: sum(1 for m in data["models"] if k not in m) for k in sorted(keys)}
    print("缺失计数:", {k: v for k, v in missing.items() if v})
    return 0


if __name__ == "__main__":
    sys.exit(main())
