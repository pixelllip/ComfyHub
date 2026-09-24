"""canon 比较：Rust 与 Kotlin 的 `/api/ai/run-kit`（同一份库 + 同一份 storage）。

规则：
1. 所有字段 canon（键排序）后逐字段比；
2. `systemPrompt` 里的 **当前时间那一行**是唯一允许不同的东西（两次调用不同时刻），
   比之前把它换成占位符 —— **格式必须一致**（Java `Instant.toString()` 的规则：
   小数位 0/3/6/9 且去尾零）。

用法：python .run/runkit/compare.py
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
KOTLIN = ROOT / ".run/runkit/baseline/kotlin-run-kit.json"
RUST = ROOT / ".run/runkit/rust-run-kit.json"

TIME_LINE = re.compile(r"^- 当前时间: (.+)$", re.M)
INSTANT = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,9})?Z$")


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
    km, rm = TIME_LINE.search(kp), TIME_LINE.search(rp)
    if not km or not rm:
        problems.append("systemPrompt 里找不到『当前时间』那一行")
    else:
        if not INSTANT.match(km.group(1)):
            problems.append("Kotlin 的时间格式不对：%r" % km.group(1))
        if not INSTANT.match(rm.group(1)):
            problems.append("Rust 的时间格式不对：%r" % rm.group(1))
        if len(km.group(1)) != len(rm.group(1)) and (km.group(1).count(".") != rm.group(1).count(".")):
            problems.append("时间格式（小数位规则）不同：%r vs %r" % (km.group(1), rm.group(1)))
        kp2 = TIME_LINE.sub("- 当前时间: <TIME>", kp, count=1)
        rp2 = TIME_LINE.sub("- 当前时间: <TIME>", rp, count=1)
        if kp2 != rp2:
            problems.append("systemPrompt 不同（已归一化时间）")
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
    print("✅ 零差异（systemPrompt %d 字符，已归一化『当前时间』那一行）" % len(kp))
    return 0


if __name__ == "__main__":
    sys.exit(main())
