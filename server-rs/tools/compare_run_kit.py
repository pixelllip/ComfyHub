"""canon 比较：Rust 与 Kotlin 的 `/api/ai/run-kit`（同一份库 + 同一份 storage）。

规则：
1. 所有字段 canon（键排序）后逐字段比；
2. `systemPrompt` 里**易变的那一行**是唯一允许不同的东西，比之前换成占位符。
   ⚠ 那一行的**拼写两侧不一样**，而且是有意的：冻结的 Kotlin 基线写的是
   `- 当前时间: <带毫秒的 ISO>`（`Instant.toString()`），2026-10 起 Rust 写的是
   `- 当前日期: YYYY-MM-DD`。所以这里**两个拼写都认**，归一化时连字段名一起换掉 ——
   否则工具会报"找不到那一行"这种假差异。改名的理由见 `src/ai/system_prompt.rs` 文件头：
   粗到"天"才能让**同一天内的后续 Run** 复用同一份系统提示（前缀缓存），
   而系统提示后面紧跟的是整段对话历史。

用法：python server-rs/tools/compare_run_kit.py
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
KOTLIN = ROOT / ".run/runkit/baseline/kotlin-run-kit.json"
RUST = ROOT / ".run/runkit/rust-run-kit.json"

# 组 1 是字段名（含 `: `），组 2 是值 —— 归一化时**两个都要换**。
STAMP_LINE = re.compile(r"^(- 当前(?:时间|日期): )(.+)$", re.M)
INSTANT = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,9})?Z$")
DAY = re.compile(r"^\d{4}-\d{2}-\d{2}$")


def canon(value):
    if isinstance(value, dict):
        return {k: canon(v) for k, v in sorted(value.items())}
    if isinstance(value, list):
        return [canon(v) for v in value]
    return value


def main():
    k = json.loads(KOTLIN.read_text(encoding="utf-8"))
    r = json.loads(RUST.read_text(encoding="utf-8"))

    problems = []
    if list(k.keys()) != list(r.keys()):
        problems.append("顶层键不同：%s vs %s" % (list(k.keys()), list(r.keys())))

    for key in k:
        if key == "systemPrompt":
            continue
        if canon(k[key]) != canon(r[key]):
            problems.append("字段 %s 不同" % key)
            if key == "models":
                kn = {m["providerId"] + "/" + m["modelId"]: m for m in k[key]}
                rn = {m["providerId"] + "/" + m["modelId"]: m for m in r[key]}
                only_k = sorted(set(kn) - set(rn))
                only_r = sorted(set(rn) - set(kn))
                problems.append("  只在 Kotlin: %s / 只在 Rust: %s" % (only_k[:5], only_r[:5]))
                for name in sorted(set(kn) & set(rn)):
                    if canon(kn[name]) != canon(rn[name]):
                        problems.append("  条目不同 %s：" % name)
                        problems.append("    kotlin=%s" % json.dumps(kn[name], ensure_ascii=False, sort_keys=True))
                        problems.append("    rust  =%s" % json.dumps(rn[name], ensure_ascii=False, sort_keys=True))
                        break
            elif isinstance(k[key], str):
                problems.append("  kotlin=%r" % k[key][:200])
                problems.append("  rust  =%r" % r[key][:200])

    kp, rp = k["systemPrompt"], r["systemPrompt"]
    km, rm = STAMP_LINE.search(kp), STAMP_LINE.search(rp)
    if not km or not rm:
        problems.append("systemPrompt 里找不到『当前时间 / 当前日期』那一行")
    else:
        for label, m in (("Kotlin", km), ("Rust", rm)):
            if not (INSTANT.match(m.group(2)) or DAY.match(m.group(2))):
                problems.append("%s 的日期/时间格式不对：%r" % (label, m.group(2)))
        # 归一化：字段名与值一起换（两侧的名字与粒度本来就允许不同，见文件头）
        kp2 = STAMP_LINE.sub(r"\1<STAMP>", kp, count=1)
        rp2 = STAMP_LINE.sub(r"\1<STAMP>", rp, count=1)
        if kp2 != rp2:
            problems.append("systemPrompt 不同（已归一化日期那一行）")
            # 找出第一处差异，给出上下文
            for i, (a, b) in enumerate(zip(kp2, rp2)):
                if a != b:
                    problems.append("  第一处差异 @%d:" % i)
                    problems.append("    kotlin=%r" % kp2[max(0, i - 80):i + 80])
                    problems.append("    rust  =%r" % rp2[max(0, i - 80):i + 80])
                    break
            else:
                problems.append("  长度不同：kotlin=%d rust=%d" % (len(kp2), len(rp2)))

    if problems:
        print("❌ 有差异：")
        for line in problems:
            print(line)
        return 1
    print("✅ 零差异（systemPrompt %d 字符，已归一化『当前时间 / 当前日期』那一行）" % len(kp))
    return 0


if __name__ == "__main__":
    sys.exit(main())
