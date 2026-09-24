"""验证 kitVersion 的内容签名公式：Python 按 Kotlin `RunKit.Signature.canonical()` 重算，
与库里 `app_settings['ai.runkit.hash']` 比对。

这一步很关键：如果 Rust 侧算出的 hash 与 Kotlin 不同，两端会**互相把版本号顶上去**
（各自看到"签名变了"→ bump → 对方又看到变了 → 再 bump）。

用法：python .run/runkit/check_signature.py
"""

import hashlib
import json
import sqlite3
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BASELINE = ROOT / ".run/runkit/baseline/kotlin-run-kit.json"
DB = ROOT / ".run/runkit/db/comfyhub.db"


def main():
    d = json.loads(BASELINE.read_text(encoding="utf-8"))
    # Kotlin 的 toolsArray 就是 [{name, description, inputSchema, scope}]，键按插入序
    tools_array = [
        {"name": t["name"], "description": t["description"], "inputSchema": t["inputSchema"], "scope": t["scope"]}
        for t in d["tools"]
    ]
    tools_json = json.dumps(tools_array, ensure_ascii=False, separators=(",", ":"))
    budget_json = json.dumps(d["inlineBudget"], ensure_ascii=False, separators=(",", ":"))
    canonical = "\n".join(
        [
            "promptVersion=%s" % d["promptVersion"],
            "nsfw=%s" % ("true" if d["nsfwEnabled"] else "false"),
            "nsfwPromptVersion=nsfw-v2",
            "tools=%s" % tools_json,
            "banner=%s" % d["untrustedBanner"],
            "budget=%s" % budget_json,
            "titleInstruction=%s" % d["titleInstruction"],
            "continueInstruction=%s" % d["continueInstruction"],
        ]
    )
    got = hashlib.sha256(canonical.encode("utf-8")).hexdigest()
    conn = sqlite3.connect(str(DB))
    stored = conn.execute("select v from app_settings where k='ai.runkit.hash'").fetchone()
    version = conn.execute("select v from app_settings where k='ai.runkit.version'").fetchone()
    print("库里的 version:", version[0] if version else None)
    print("库里的 hash   :", stored[0] if stored else None)
    print("重算的 hash   :", got)
    print("一致:", bool(stored) and stored[0] == got)
    if not stored or stored[0] != got:
        print("--- canonical 的字符数与首尾 ---")
        print(len(canonical))
        print(repr(canonical[:120]))
        print(repr(canonical[-120:]))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
