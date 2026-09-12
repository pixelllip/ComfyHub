#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
生成一张带 ComfyUI 风格元数据的 PNG。

真实的 ComfyUI 存 PNG 时会写两个 tEXt 块：
    prompt   -> API 格式节点图（参数）
    workflow -> 界面格式工作流
这里手工造一张，用来验证：
  · PngMeta 能否读出这两个块
  · 「导入已有产物」能否据此自动建提示词并关联产物

用法：
    python make_png.py <输出路径> [--size 1024] [--color 30,90,180]
"""

import argparse
import json
import os
import struct
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fake_comfy import GRAPH, WORKFLOW  # noqa: E402


def _chunk(tag: bytes, data: bytes) -> bytes:
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def _text_chunk(key: str, value: str) -> bytes:
    return _chunk(b"tEXt", key.encode("latin-1") + b"\x00" + value.encode("utf-8"))


def make_png(width: int, height: int, rgb) -> bytes:
    row = b"\x00" + bytes(rgb) * width
    raw = row * height
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + _chunk(b"IHDR", ihdr)
        + _text_chunk("prompt", json.dumps(GRAPH, ensure_ascii=False))
        + _text_chunk("workflow", json.dumps(WORKFLOW, ensure_ascii=False))
        + _chunk(b"IDAT", zlib.compress(raw, 6))
        + _chunk(b"IEND", b"")
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("path")
    parser.add_argument("--size", type=int, default=1024)
    parser.add_argument("--color", default="30,90,180")
    args = parser.parse_args()

    rgb = tuple(int(x) for x in args.color.split(","))
    os.makedirs(os.path.dirname(os.path.abspath(args.path)), exist_ok=True)
    data = make_png(args.size, args.size, rgb)
    with open(args.path, "wb") as fh:
        fh.write(data)
    print("[make-png] %s (%d bytes, %dx%d)" % (args.path, len(data), args.size, args.size))


if __name__ == "__main__":
    main()
