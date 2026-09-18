#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
假的 ComfyUI，用来端到端验证 ComfyHub 的「自动捕获」链路与「AI 提交任务」。

它只实现会用到的几个接口：
    GET  /history    -> 一条已经执行完的运行（参数节点图 + 工作流 + 产物文件名）
    GET  /history/<prompt_id> -> 同上，但只给那一条（提交任务后按 prompt_id 轮询用）
    GET  /queue      -> 空队列
    GET  /view       -> 把 output 目录里的文件吐出来
    GET  /system_stats
    POST /prompt     -> 收下一次提交，**用它自己的节点图**造一条已完成的运行
                        （用户建议 ①：AI 提交任务这条链路要能端到端验）

这样不需要真的跑一次生成，就能验证：
  · 后端轮询能否发现新运行
  · 参数（seed / steps / cfg / 模型 / LoRA / 宽高）解析是否正确
  · 工作流是否原样存进库
  · 产物文件（本地读取 或 HTTP 下载）是否入库并与提示词关联
  · 重复轮询是否幂等
  · `comfy_submit` 提交→轮询→入库→拿到 mediaIds 的整条链路

用法：
    python fake_comfy.py [--port 8188] [--output <dir>] [--prompt-id <id>]
"""

import argparse
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

HERE = os.path.dirname(os.path.abspath(__file__))

# --- 一条典型的生成：SDXL + LoRA + 1024x1024 ---------------------------------

CHECKPOINT = "e2e_sdxl_base_1.0.safetensors"
LORA = "e2e_detail_tweaker.safetensors"
POSITIVE = "e2e test prompt, neon alley at night, cinematic lighting"
NEGATIVE = "lowres, watermark"
SEED = 20260101
STEPS = 24
CFG = 6.5
WIDTH = 1024
HEIGHT = 1024

GRAPH = {
    "4": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": CHECKPOINT}},
    "5": {"class_type": "EmptyLatentImage",
          "inputs": {"width": WIDTH, "height": HEIGHT, "batch_size": 2}},
    "6": {"class_type": "CLIPTextEncode", "inputs": {"text": POSITIVE, "clip": ["4", 1]}},
    "7": {"class_type": "CLIPTextEncode", "inputs": {"text": NEGATIVE, "clip": ["4", 1]}},
    "8": {"class_type": "LoraLoader",
          "inputs": {"lora_name": LORA, "strength_model": 0.8, "strength_clip": 0.8,
                     "model": ["4", 0], "clip": ["4", 1]}},
    "3": {"class_type": "KSampler",
          "inputs": {"seed": SEED, "steps": STEPS, "cfg": CFG,
                     "sampler_name": "dpmpp_2m", "scheduler": "karras", "denoise": 1.0,
                     "model": ["8", 0], "positive": ["6", 0], "negative": ["7", 0],
                     "latent_image": ["5", 0]}},
    "9": {"class_type": "SaveImage",
          "inputs": {"filename_prefix": "e2e/e2e_capture", "images": ["3", 0]}},
}

# 界面格式工作流（真实 ComfyUI 会把 UI 里的 workflow 塞进 extra_pnginfo）
WORKFLOW = {
    "last_node_id": 9,
    "last_link_id": 9,
    "nodes": [
        {"id": 3, "type": "KSampler", "title": "e2e-node",
         "widgets_values": [SEED, "fixed", STEPS, CFG, "dpmpp_2m", "karras", 1.0]},
        {"id": 9, "type": "SaveImage", "widgets_values": ["e2e/e2e_capture"]},
    ],
    "links": [],
    "extra": {"comfyhub_e2e": True},
}


# 节点定义表（`GET /object_info` 的极简版）。
#
# 只覆盖"界面格式 → API 节点图"这条转换会问到的几个字段：
# `input.required` 的**声明顺序**（决定 widgets_values 的位置对应哪个参数）、
# 组合框（值是一个数组）与 `control_after_generate`（种子后面那个下拉框会多占一个槽位）。
OBJECT_INFO = {
    "CheckpointLoaderSimple": {
        "input": {"required": {"ckpt_name": [[CHECKPOINT, "other.safetensors"]]}}
    },
    "CLIPTextEncode": {
        "input": {"required": {"text": ["STRING", {"multiline": True}], "clip": ["CLIP"]}}
    },
    "EmptyLatentImage": {
        "input": {"required": {
            "width": ["INT", {"default": 512}],
            "height": ["INT", {"default": 512}],
            "batch_size": ["INT", {"default": 1}],
        }}
    },
    "LoraLoader": {
        "input": {"required": {
            "lora_name": [[LORA]],
            "strength_model": ["FLOAT", {"default": 1.0}],
            "strength_clip": ["FLOAT", {"default": 1.0}],
            "model": ["MODEL"],
            "clip": ["CLIP"],
        }}
    },
    "KSampler": {
        "input": {"required": {
            "model": ["MODEL"],
            "seed": ["INT", {"default": 0, "control_after_generate": True}],
            "steps": ["INT", {"default": 20}],
            "cfg": ["FLOAT", {"default": 7.0}],
            "sampler_name": [["euler", "dpmpp_2m", "ddim"]],
            "scheduler": [["normal", "karras"]],
            "positive": ["CONDITIONING"],
            "negative": ["CONDITIONING"],
            "latent_image": ["LATENT"],
            "denoise": ["FLOAT", {"default": 1.0}],
        }}
    },
    "SaveImage": {
        "input": {
            "required": {"images": ["IMAGE"]},
            "optional": {"filename_prefix": ["STRING", {"default": "ComfyUI"}]},
        }
    },
}


def pick_output_node(graph):
    """挑出这张图的产物节点。

    真实工作流的节点 id 不一定是数字（这个库里的 H3 工作流用的是 "out"），
    而且产物节点也不一定叫 SaveImage（VHS_VideoCombine、SaveAnimatedWEBP…）。
    判据按可能性从高到低：名字里带 save/combine → 没有任何下游引用 → 第一个节点。
    """
    if not isinstance(graph, dict) or not graph:
        return None
    preferred = ("save", "combine", "preview", "export")
    for nid, node in graph.items():
        cls = str((node or {}).get("class_type", "")).lower()
        if any(k in cls for k in preferred):
            return nid
    referenced = set()
    for node in graph.values():
        for value in ((node or {}).get("inputs") or {}).values():
            if isinstance(value, list) and value and isinstance(value[0], str):
                referenced.add(value[0])
    leaves = [nid for nid in graph.keys() if nid not in referenced]
    return leaves[0] if leaves else next(iter(graph.keys()))


def build_history(prompt_id: str, filename: str, extra_files=None):
    images = [{"filename": filename, "subfolder": "", "type": "output"}]
    if extra_files:
        images.extend(extra_files)
    return {
        prompt_id: {
            "prompt": [
                GRAPH,
                {"client_id": "e2e-client",
                 "extra_pnginfo": {"workflow": WORKFLOW}},
                ["9"],
            ],
            "outputs": {"9": {"images": images}},
            "status": {"status_str": "success", "completed": True, "messages": []},
        }
    }


class Handler(BaseHTTPRequestHandler):
    history = {}
    output_dir = ""
    version = "0.34.2-fake"
    filename = "e2e_capture_00001_.png"
    # 收到过的提交（POST /prompt），供测试脚本检查"到底提交了什么图"
    submitted = []

    def log_message(self, *args):  # 静音，免得刷屏
        pass

    def _json(self, payload):
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path == "/history":
            # 真实 ComfyUI 支持 ?max_items=，这里忽略即可
            self._json(self.history)
        elif parsed.path == "/object_info":
            # 真实 ComfyUI 的节点定义表。ComfyHub 把**界面格式**工作流转成 API 节点图时要用它
            # （widgets_values 只有位置、没有参数名，顺序只能从这里拿）——
            # 少了这个端点，`comfy_load_workflow` 那条链路在测试里就只能报"连不上"。
            self._json(OBJECT_INFO)
        elif parsed.path.startswith("/history/"):
            # 提交任务之后 ComfyHub 会按 prompt_id 精确轮询（ComfySubmitter.historyEntry）
            key = parsed.path[len("/history/"):]
            entry = self.history.get(key)
            self._json({key: entry} if entry else {})
        elif parsed.path == "/queue":
            self._json({"queue_running": [], "queue_pending": []})
        elif parsed.path == "/__submitted":
            # 测试脚本用它检查"到底提交了什么图"，不参与生成
            self._json({"runs": Handler.submitted})
        elif parsed.path == "/system_stats":
            self._json({"system": {"comfyui_version": self.version, "os": sys.platform}})
        elif parsed.path == "/view":
            query = parse_qs(parsed.query)
            name = (query.get("filename") or [""])[0]
            subfolder = (query.get("subfolder") or [""])[0]
            path = os.path.join(self.output_dir, subfolder, name) if subfolder \
                else os.path.join(self.output_dir, name)
            if os.path.isfile(path):
                with open(path, "rb") as fh:
                    data = fh.read()
                self.send_response(200)
                self.send_header("Content-Type", "image/png")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
            else:
                self.send_error(404, "not found: " + name)
        elif parsed.path == "/":
            self._json({"app": "fake-comfyui", "history": list(self.history.keys())})
        else:
            self.send_error(404)

    def do_POST(self):
        parsed = urlparse(self.path)
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        if parsed.path == "/__submitted":
            # 测试脚本用它检查"到底提交了什么图"，不参与生成
            self._json({"runs": Handler.submitted})
            return
        if parsed.path != "/prompt":
            self.send_error(404)
            return
        try:
            body = json.loads(raw.decode("utf-8"))
        except Exception as exc:  # noqa: BLE001
            self.send_error(400, "bad json: %s" % exc)
            return
        graph = body.get("prompt") or {}
        if not isinstance(graph, dict) or not graph:
            self.send_error(400, "empty prompt")
            return

        # 每一次提交都当成"立刻跑完"：用**它自己的节点图**造 history，
        # 这样 ComfyHub 解析出来的提示词 / 参数就是模型实际提交的那一份，
        # 而不是 fake_comfy 里写死的那条 —— 否则这个测试证明不了任何事。
        prompt_id = "e2e-submit-%04d" % (len(Handler.submitted) + 1)
        Handler.submitted.append({"promptId": prompt_id, "graph": graph})
        out_node = pick_output_node(graph)
        Handler.history[prompt_id] = {
            "prompt": [graph, {"client_id": body.get("client_id") or "e2e", "extra_pnginfo": {"workflow": WORKFLOW}}, [out_node]],
            "outputs": {out_node: {"images": [{"filename": Handler.filename, "subfolder": "", "type": "output"}]}},
            "status": {"status_str": "success", "completed": True, "messages": []},
        }
        self._json({"prompt_id": prompt_id, "number": len(Handler.submitted), "node_errors": {}})


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=8188)
    parser.add_argument("--output", default=os.path.join(HERE, "output"))
    parser.add_argument("--prompt-id", default="e2e-capture-0001")
    parser.add_argument("--filename", default="e2e_capture_00001_.png")
    args = parser.parse_args()

    Handler.output_dir = args.output
    Handler.filename = args.filename
    Handler.history = build_history(args.prompt_id, args.filename)

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print("[fake-comfyui] listening on http://127.0.0.1:%d (prompt_id=%s, output=%s)"
          % (args.port, args.prompt_id, args.output), flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
