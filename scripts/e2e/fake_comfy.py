#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
假的 ComfyUI，用来端到端验证 ComfyHub 的「自动捕获」链路。

它只实现自动捕获会用到的几个接口：
    GET /history    -> 一条已经执行完的运行（参数节点图 + 工作流 + 产物文件名）
    GET /queue      -> 空队列
    GET /view       -> 把 output 目录里的文件吐出来
    GET /system_stats

这样不需要真的跑一次生成，就能验证：
  · 后端轮询能否发现新运行
  · 参数（seed / steps / cfg / 模型 / LoRA / 宽高）解析是否正确
  · 工作流是否原样存进库
  · 产物文件（本地读取 或 HTTP 下载）是否入库并与提示词关联
  · 重复轮询是否幂等

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
        elif parsed.path == "/queue":
            self._json({"queue_running": [], "queue_pending": []})
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=8188)
    parser.add_argument("--output", default=os.path.join(HERE, "output"))
    parser.add_argument("--prompt-id", default="e2e-capture-0001")
    parser.add_argument("--filename", default="e2e_capture_00001_.png")
    args = parser.parse_args()

    Handler.output_dir = args.output
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
