"""假 MCP 服务器：给 Kotlin 侧的 McpStdioClient 做端到端回归。

**形状照着真 comfy-mcp 抄**（2026-09-21 实测的那一套）：
  initialize → tools/list → tools/call，一律 JSON-RPC 2.0、一行一条、UTF-8。
和真服务器的差别只有规模：这里只有 3 个工具，但把三种结果都覆盖了 ——
正常文本、`isError=true`、以及"有 structuredContent"。

为什么要这个文件：MCP 客户端的坑全在协议上（id 对不上、通知没有 id、
stdout 混进噪音行、stderr 不排空会死锁），只有真起一个子进程才能抓到。

用法（测试里用）：python fake_mcp_server.py [--noisy]
  --noisy 会在 stdout 里先插几行非 JSON 噪音，验证客户端"跳过噪音但不崩"。
"""
import json
import sys

TOOLS = [
    {
        "name": "system_stats",
        "description": "Read the live local ComfyUI's VRAM per device and system RAM.",
        "inputSchema": {"type": "object", "properties": {}, "title": "system_statsArguments"},
    },
    {
        "name": "run_workflow",
        "description": "Run a ComfyUI workflow JSON.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "workflow_path": {"type": "string"},
                "wait": {"type": "boolean"},
            },
            "required": ["workflow_path"],
        },
    },
    {
        "name": "install_node",
        "description": "Install custom node packs into the LOCAL ComfyUI.",
        "inputSchema": {"type": "object", "properties": {"names": {"type": "array"}}, "required": ["names"]},
    },
]


def send(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def main() -> int:
    noisy = "--noisy" in sys.argv
    if noisy:
        # 真实服务器也会往 stdout 漏东西（警告 / banner）—— 客户端必须跳过而不是崩
        sys.stdout.write("warning: something unimportant\n")
        sys.stdout.flush()

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue

        method = msg.get("method")
        mid = msg.get("id")

        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": mid,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {"listChanged": False}},
                    "serverInfo": {"name": "fake-comfy-mcp", "version": "0.0.1"},
                    "instructions": "FAKE SERVER: call system_stats first.",
                },
            })
        elif method == "notifications/initialized":
            # 通知没有 id，**不该回**任何东西；回了就会污染协议流
            continue
        elif method == "tools/list":
            send({"jsonrpc": "2.0", "id": mid, "result": {"tools": TOOLS}})
        elif method == "tools/call":
            params = msg.get("params") or {}
            name = params.get("name")
            args = params.get("arguments") or {}
            if name == "system_stats":
                send({
                    "jsonrpc": "2.0",
                    "id": mid,
                    "result": {
                        "content": [{"type": "text", "text": '{"devices":[{"vram_free": 123456, "name": "RTX 4060"}]}'}],
                        "isError": False,
                        "structuredContent": {"devices": [{"vram_free": 123456}]},
                    },
                })
            elif name == "run_workflow":
                # 把收到的参数原样回显 —— 测试据此验证"参数是原样透传的"
                send({
                    "jsonrpc": "2.0",
                    "id": mid,
                    "result": {
                        "content": [{"type": "text", "text": "echo:" + json.dumps(args, ensure_ascii=False, sort_keys=True)}],
                        "isError": False,
                    },
                })
            elif name == "install_node":
                # 服务端说"这次执行失败了"—— 客户端要翻成 ToolFailure，而不是成功
                send({
                    "jsonrpc": "2.0",
                    "id": mid,
                    "result": {"content": [{"type": "text", "text": "refused: install_node is not allowed"}], "isError": True},
                })
            else:
                send({
                    "jsonrpc": "2.0",
                    "id": mid,
                    "error": {"code": -32602, "message": "unknown tool: %s" % name},
                })
        elif method == "tools/slow":
            # 用来自测超时：永不回复
            continue
        else:
            if mid is not None:
                send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "method not found"}})

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
