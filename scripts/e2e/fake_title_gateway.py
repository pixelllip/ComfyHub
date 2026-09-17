"""标题自动总结（用户建议 ③）的端到端假网关：只为这一条链路服务。

行为刻意做得像**真实的 SSE 网关**：
  1. 收到第一轮请求（上下文里没有 assistant）→ 流式吐 `[标题]赛博朋克少女[/标题]` 再吐正文；
  2. 第二次请求带上了历史 → 只吐正文（证明"只有第一问才要标题"）。
把请求体录音到 `--log`，供 PowerShell 侧断言"后端真的把标题要求发下去了"。
"""

import argparse
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

LOG = []
LOCK = threading.Lock()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):  # 静音
        pass

    def _json(self, payload, status=200):
        raw = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _record(self, entry):
        with LOCK:
            LOG.append(entry)

    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/__log":
            with LOCK:
                self._json({"entries": LOG})
            return
        if path == "/v1/models":
            self._json({"data": [{"id": "fake-title-model", "object": "model"}]})
            return
        self._json({"error": "not found"}, 404)

    def do_POST(self):
        path = urlparse(self.path).path
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        if path != "/v1/chat/completions":
            self._json({"error": "not found: " + path}, 404)
            return
        body = json.loads(raw.decode("utf-8"))
        messages = body.get("messages") or []
        tools = body.get("tools") or []
        system_text = "\n".join(
            (m.get("content") if isinstance(m.get("content"), str) else "")
            for m in messages if m.get("role") == "system"
        )
        has_assistant = any(m.get("role") == "assistant" for m in messages)
        last = messages[-1] if messages else {}
        user_text = ""
        for m in reversed(messages):
            if m.get("role") == "user":
                user_text = m.get("content") if isinstance(m.get("content"), str) else ""
                break
        self._record({
            "roles": [m.get("role") for m in messages],
            "askedForTitle": "[标题]" in system_text,
            "hasAssistantHistory": has_assistant,
            "toolNames": [((t.get("function") or {}).get("name")) for t in tools],
        })

        # 模板 A：走「找工作流」这条工具链（不真的提交，只看结果能不能落进库）。
        # 关键词从用户那句话里现取（跟在「找找看」后面），这样换个库也能用。
        if "找找看" in user_text and last.get("role") != "tool":
            keyword = user_text.split("找找看", 1)[1].strip().split(" ")[0] or "e2e"
            self._stream_tool("comfy_find_workflow", {"query": keyword, "limit": 2, "includeGraph": True})
            return
        # 模板 C：真的提交一次任务（用户建议 ① 的端到端）。promptId 从「提交 82」里取。
        # 覆盖的参数路径按**工作流自己的节点编号**给（节点 id 不一定是数字，
        # 这个库里的工作流用的是 "sch" 这类字符串 id —— 与 ComfyUI 界面上看到的一致）。
        if "提交" in user_text and last.get("role") != "tool":
            digits = "".join(ch for ch in user_text.split("提交", 1)[1] if ch.isdigit())
            prompt_id = int(digits or "0")
            self._stream_tool("comfy_submit", {
                "promptId": prompt_id,
                "overrides": {"sch.steps": "12"},
                "title": "E2E 提交验证",
                "waitSeconds": 60,
            })
            return
        # 模板 B：普通问答（顺带验标题）
        prefix = "[标题]赛博朋克少女[/标题]\n\n" if not has_assistant else ""
        text = prefix + "好的，我按赛博朋克少女的方向来。"
        self._sse(text)

    def _stream_tool(self, name, arguments):
        self._sse_frames([
            {"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call-e2e-1",
                                                    "type": "function",
                                                    "function": {"name": name, "arguments": ""}}]},
                          "index": 0}]},
            {"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {
                "arguments": json.dumps(arguments, ensure_ascii=False)}}]}, "index": 0}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls", "index": 0}]},
        ])

    def _sse_frames(self, frames):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        for payload in frames:
            self.wfile.write(("data: " + json.dumps(payload, ensure_ascii=False) + "\n\n").encode("utf-8"))
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def _sse(self, text):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        # 刻意按"每 3 个字一片"发：标记一定会被切开，逼后端的状态机真的处理分片
        for i in range(0, len(text), 3):
            chunk = text[i:i + 3]
            payload = {"choices": [{"delta": {"content": chunk}, "index": 0}]}
            self.wfile.write(("data: " + json.dumps(payload, ensure_ascii=False) + "\n\n").encode("utf-8"))
            self.wfile.flush()
        done = {"choices": [{"delta": {}, "finish_reason": "stop", "index": 0}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 20}}
        self.wfile.write(("data: " + json.dumps(done) + "\n\n").encode("utf-8"))
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=8798)
    args = parser.parse_args()
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
