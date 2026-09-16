#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
假的 OpenAI 兼容网关（流式 + 工具调用），用来端到端验证 ComfyHub 的
「AI 工具循环（M4）+ Skills（M5）」链路。**不需要真实 API Key，也不出网。**

它只实现三件事：

    GET  /v1/models                -> 一个模型：fake-tools-model
    POST /v1/chat/completions      -> SSE；**按请求内容决定回什么**
    GET  /__log                    -> 收到过的请求记录（测试脚本据此断言"工具结果被喂回去了"）

分流规则（看 `messages`）：

    · 最后一条是 role=tool           -> 回一句中文正文（循环终止）
    · 最后一条 user 文本含「注册」    -> register_skill
    · 含「加载」                     -> load_skill
    · 含「越界」                     -> write_file（storage/e2e-hack.txt，应被权限策略拒绝）
    · 含「目录内」                    -> write_file（comfyui/e2e-ok.txt，应成功）
    · 含「同步」                     -> comfy_sync_history（需要审批）
    · 含「状态」                     -> comfy_get_status（只读）
    · 其它                          -> 普通正文

SSE 线格式刻意做成**真网关的样子**：第一片带 index/id/type/function.name +
空 `function.arguments`，之后只带 index + `function.arguments` 片段，
并且把参数 JSON 拆成 4 段（一定有一段切在某个 value 中间），
用来验证后端的 ToolCallAccumulator 能正确拼回去。

用法：
    python fake_openai.py [--port 8799]
"""

import argparse
import json
import re
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

MODEL_ID = "fake-tools-model"

# --- 每个场景要发出的工具调用（arguments 是 JSON 字符串） ----------------------

SKILL_ARGS = {
    "name": "e2e-tool-demo",
    "description": "E2E 测试用 skill",
    "whenToUse": "只在自动化测试里",
    "content": "# 演示\n步骤一\n步骤二",
}


def _args(obj) -> str:
    return json.dumps(obj, ensure_ascii=False)


SCENARIOS = [
    # (关键词, 工具名, arguments JSON 字符串)
    ("注册", "register_skill", _args(SKILL_ARGS)),
    ("加载", "load_skill", _args({"name": "e2e-tool-demo"})),
    ("记住", "remember", _args({"content": "E2E 记一条：用户偏好 4:3 画幅"})),
    ("越界", "write_file", _args({"path": "storage/e2e-hack.txt", "content": "should be denied"})),
    ("目录内", "write_file", _args({"path": "comfyui/e2e-ok.txt", "content": "hello from tool"})),
    ("同步", "comfy_sync_history", _args({})),
    ("状态", "comfy_get_status", _args({})),
]


def split_arguments(payload: str, parts: int = 4):
    """把参数 JSON 切成 parts 段（至少 3 段），故意在 value 中间断开。"""
    n = len(payload)
    if n < parts:
        return [payload] if payload else []
    cuts = sorted({round(n * i / parts) for i in range(1, parts)})
    out, prev = [], 0
    for c in cuts:
        out.append(payload[prev:c])
        prev = c
    out.append(payload[prev:])
    return [p for p in out if p != ""]


def one_line(text: str, limit: int = 60) -> str:
    flat = re.sub(r"\s+", " ", text or "").strip()
    return flat if len(flat) <= limit else flat[:limit] + "…"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    log_entries = []
    log_lock = threading.Lock()
    call_seq = 0
    call_lock = threading.Lock()

    # ------------------------------------------------------------------ 工具

    def log_message(self, *args):
        pass

    def _json(self, payload, status=200):
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def _record(self, entry):
        with Handler.log_lock:
            entry["seq"] = len(Handler.log_entries) + 1
            Handler.log_entries.append(entry)

    def _next_call_id(self, tool: str) -> str:
        with Handler.call_lock:
            Handler.call_seq += 1
            n = Handler.call_seq
        return "call_%s_%d" % (tool, n)

    # ------------------------------------------------------------------- GET

    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/v1/models":
            self._record({"method": "GET", "path": path, "kind": "models"})
            self._json({"object": "list", "data": [{"id": MODEL_ID, "object": "model",
                                                    "owned_by": "fake"}]})
        elif path == "/__log":
            with Handler.log_lock:
                self._json(list(Handler.log_entries))
        elif path in ("/", "/health"):
            self._json({"app": "fake-openai", "model": MODEL_ID,
                        "requests": len(Handler.log_entries)})
        else:
            self._json({"error": "not found"}, 404)

    # ------------------------------------------------------------------ POST

    def do_POST(self):
        path = urlparse(self.path).path
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        if path == "/__reset":
            with Handler.log_lock:
                Handler.log_entries.clear()
            self._json({"ok": True})
            return
        if path != "/v1/chat/completions":
            self._json({"error": "not found: " + path}, 404)
            return

        try:
            body = json.loads(raw.decode("utf-8"))
        except Exception as exc:  # noqa: BLE001
            self._json({"error": "bad json: %s" % exc}, 400)
            return

        messages = body.get("messages") or []
        tools = body.get("tools") or []
        roles = [m.get("role") for m in messages]
        last = messages[-1] if messages else {}
        assistant_tool_calls = any(m.get("role") == "assistant" and m.get("tool_calls")
                                   for m in messages)
        tool_results = [m for m in messages if m.get("role") == "tool"]
        system_text = "\n".join((m.get("content") or "") for m in messages
                                if m.get("role") == "system")
        # 「长期记忆」是每次 Run 现渲染进系统提示的：这里把是否带上、带没带上某条记进日志，
        # 测试脚本据此断言"AI 记得住、也看得到"（不用去猜后端实现）。
        memory_marker = "长期记忆"
        tool_names = [((t.get("function") or {}).get("name")) for t in tools]

        entry = {
            "method": "POST",
            "path": path,
            "kind": "chat",
            "model": body.get("model"),
            "stream": bool(body.get("stream")),
            "roles": roles,
            "toolsPresent": bool(tools),
            "toolCount": len(tools),
            "toolNames": tool_names,
            "hasRememberTool": "remember" in tool_names,
            "systemHasMemorySection": memory_marker in system_text,
            "systemMemoryHasProbe": "E2E 记一条" in system_text,
            "assistantHasToolCall": assistant_tool_calls,
            "toolResultCount": len(tool_results),
            "lastRole": last.get("role"),
        }

        # 1) 工具结果已回填 -> 用正文收尾（循环终止）
        if last.get("role") == "tool":
            replied = one_line(last.get("content"))
            entry["decision"] = "final_text"
            entry["replyPreview"] = replied
            self._record(entry)
            text = "工具已返回：%s。" % replied
            self._stream_text(text, prompt_tokens=sum(len(m.get("content") or "") for m in messages))
            return

        # 2) 按最后一条 user 文本分流
        user_text = ""
        for m in reversed(messages):
            if m.get("role") == "user":
                user_text = m.get("content") or ""
                break
        entry["userText"] = user_text

        for keyword, tool_name, arguments in SCENARIOS:
            if keyword in user_text:
                entry["decision"] = "tool:" + tool_name
                self._record(entry)
                self._stream_tool(tool_name, arguments)
                return

        entry["decision"] = "text"
        self._record(entry)
        self._stream_text("（假网关）收到：%s" % one_line(user_text, 40))

    # ------------------------------------------------------------------- SSE

    def _sse_start(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()

    def _chunk(self, text: str):
        data = text.encode("utf-8")
        self.wfile.write(b"%x\r\n" % len(data) + data + b"\r\n")
        self.wfile.flush()

    def _data(self, payload):
        self._chunk("data: " + json.dumps(payload, ensure_ascii=False) + "\n\n")

    def _sse_end(self):
        self._chunk("data: [DONE]\n\n")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def _envelope(self, delta, finish_reason=None, usage=None, response_id="chatcmpl-fake"):
        payload = {
            "id": response_id,
            "object": "chat.completion.chunk",
            "created": 0,
            "model": MODEL_ID,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}],
        }
        if usage is not None:
            payload["usage"] = usage
        return payload

    def _usage(self, prompt_tokens, completion_tokens):
        return {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        }

    def _stream_text(self, text: str, prompt_tokens: int = 12):
        self._sse_start()
        self._data(self._envelope({"role": "assistant", "content": ""}))
        # 正文也切成两片，顺便覆盖 text.delta 的拼接
        mid = max(1, len(text) // 2)
        for piece in (text[:mid], text[mid:]):
            if piece:
                self._data(self._envelope({"content": piece}))
        self._data(self._envelope({}, finish_reason="stop",
                                  usage=self._usage(prompt_tokens, len(text))))
        self._sse_end()

    def _stream_tool(self, name: str, arguments: str):
        call_id = self._next_call_id(name)
        pieces = split_arguments(arguments, 4)
        self._sse_start()
        self._data(self._envelope({"role": "assistant", "content": ""}))
        # 第一片：index + id + type + function.name + 空 arguments
        self._data(self._envelope({"tool_calls": [{
            "index": 0,
            "id": call_id,
            "type": "function",
            "function": {"name": name, "arguments": ""},
        }]}))
        # 后续片：只有 index + arguments 片段
        for piece in pieces:
            self._data(self._envelope({"tool_calls": [{
                "index": 0,
                "function": {"arguments": piece},
            }]}))
        self._data(self._envelope({}, finish_reason="tool_calls",
                                  usage=self._usage(24, len(arguments))))
        self._sse_end()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=8799)
    parser.add_argument("--host", default="127.0.0.1")
    args = parser.parse_args()

    ThreadingHTTPServer.daemon_threads = True
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    print("[fake-openai] listening on http://%s:%d/v1 (model=%s)"
          % (args.host, args.port, MODEL_ID), flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
