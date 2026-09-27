#!/usr/bin/env python
"""人物内部锐度尺子（拉普拉斯方差）—— 给"人变糊了"这件事一个客观判据。

为什么需要它（总纲 §15.2）：用户看图说"人变糊了"，而"真的糊了"与"风格更柔和 / 整体
降采样"在缩略图上分不开。拉普拉斯方差（variance of Laplacian，VoL）是经典的清晰度
指标：**同一块人物内部区域**上数值明显下降，才叫真的糊了。

# 口径（三条，e2e 的判据就按它写）

1. **只在人物内部量**：把近似剪影**向内腐蚀 `--erode` 像素**，只在剩下的像素上算 ——
   剪影边界上混着背景与 alpha，量到的是"边缘对比度"而不是"内部细节"；
2. **亮度取 Rec.601**（`0.299R+0.587G+0.114B`），拉普拉斯用 4 邻域核
   `[0,1,0; 1,-4,1; 0,1,0]`，取响应值的**方差**；
3. **剪影从人物图现推，而且所有被比较的图共用同一个掩码** —— 否则"谁的人更大 /
   谁的位置偏了一点"就会变成变量，比出来的差异分不清是参数还是构图。

# ⚠ 前提与代价（别拿它当万能尺）

- 要求**人物图与产物同尺寸、同位置**：用了 `-PlaceScale` / `-PlaceX` / `-PlaceY`
  之后人物被搬过，掩码就对不上了。这时它会如实报 `maskRatio`，由调用方自己判断。
- 剪影是**颜色近似**（离四角背景色的曼哈顿距离 > `--tol`），所以只对"纯色 / 浅色背景的
  人物图"成立 —— 绿幕那一档最好使；背景很花时掩码会把背景算进来（`maskRatio` 会偏大）。
  它**只用来量锐度**，不参与任何抠图/合成决策。
- 产物若是**纯合成**（一个像素都没重绘），量到的数值 = "VAE 编解码的损失"，那正是判据的
  基线（"内部锐度不许比纯合成低 20% 以上"）。

用法：
  python sharpness.py --image <产物.png> --person <人物图.png> [--erode 8] [--tol 90]
  python sharpness.py --image <产物.png> --person <人物图.png> --baseline <纯合成.png> [--max-drop 0.2]

输出（stdout，一行 JSON —— 调用方解析它，别去猜人类可读文本）：
  {"image":…, "laplacianVariance":…, "interiorPixels":…, "maskRatio":…, "meanLuma":…,
   "baseline":{"path":…, "laplacianVariance":…}, "ratio":…, "maxDrop":…, "pass": true|false}

退出码：0 = 量到了（`pass` 字段才是判据）；2 = 参数/文件/尺寸不对（拿不到数就别说通过）。
"""

import argparse
import json
import sys

import numpy as np
from PIL import Image, ImageFilter

# 拉普拉斯响应方差对**噪声**很敏感：JPEG 的块噪声会把数值抬上去。
# 所以判据只在**同一条链路的两张产物之间**比（同格式、同尺寸、同掩码），不做跨格式比较。
LAPLACIAN_KERNEL = "4 邻域"


def load_rgb(path):
    with Image.open(path) as img:
        return np.asarray(img.convert("RGB"), dtype=np.float32)


def load_person(path, size):
    """人物图 → (RGB, alpha 或 None)。尺寸不一致时缩到产物尺寸（并让调用方知道）。"""
    with Image.open(path) as img:
        alpha = None
        if img.mode in ("RGBA", "LA") or (img.mode == "P" and "transparency" in img.info):
            alpha = np.asarray(img.convert("RGBA").getchannel("A"), dtype=np.uint8)
        rgb = np.asarray(img.convert("RGB"), dtype=np.float32)
    resized = False
    if alpha is not None and alpha.shape != (size[1], size[0]):
        alpha = np.asarray(
            Image.fromarray(alpha).resize(size, Image.BILINEAR), dtype=np.uint8
        )
        resized = True
    if rgb.shape[:2] != (size[1], size[0]):
        rgb = np.asarray(
            Image.fromarray(rgb.astype(np.uint8)).resize(size, Image.BICUBIC), dtype=np.float32
        )
        resized = True
    return rgb, alpha, resized


def silhouette(person_rgb, alpha, tol):
    """近似剪影：有 alpha 就用 alpha，否则按"离四角背景色多远"判（与 Rust 同一套口径）。"""
    if alpha is not None:
        return alpha > 127
    corners = np.stack(
        [
            person_rgb[1, 1],
            person_rgb[1, -2],
            person_rgb[-2, 1],
            person_rgb[-2, -2],
        ]
    )
    bg = np.median(corners, axis=0)
    dist = np.abs(person_rgb - bg).sum(axis=2)
    return dist > tol


def erode(mask, radius):
    """向内腐蚀 `radius` 像素（半径 0 = 原样）。

    用 PIL 的 MinFilter：`size` 必须是 **奇数**，而"半径 r"对应的窗口是 `2r+1`。
    """
    if radius <= 0:
        return mask
    img = Image.fromarray((mask.astype(np.uint8) * 255))
    img = img.filter(ImageFilter.MinFilter(2 * radius + 1))
    return np.asarray(img) > 127


def luma(rgb):
    return 0.299 * rgb[..., 0] + 0.587 * rgb[..., 1] + 0.114 * rgb[..., 2]


def laplacian(gray):
    """4 邻域拉普拉斯（边缘一圈留 0；掩码已经把它排除在外了）。"""
    lap = np.zeros_like(gray)
    lap[1:-1, 1:-1] = (
        -4.0 * gray[1:-1, 1:-1]
        + gray[:-2, 1:-1]
        + gray[2:, 1:-1]
        + gray[1:-1, :-2]
        + gray[1:-1, 2:]
    )
    return lap


def measure(rgb, mask):
    interior = int(mask.sum())
    if interior == 0:
        return None
    gray = luma(rgb)
    lap = laplacian(gray)
    values = lap[mask]
    return {
        "interiorPixels": interior,
        "laplacianVariance": round(float(values.var()), 4),
        "meanLuma": round(float(gray[mask].mean()), 3),
        # 拉普拉斯的**均值**只反映直流分量（一般是 0 附近），带上是给"整块偏亮/偏暗"留个旁证
        "meanLaplacian": round(float(values.mean()), 5),
    }


def main(argv=None):
    ap = argparse.ArgumentParser(add_help=True, description="人物内部锐度（拉普拉斯方差）")
    ap.add_argument("--image", required=True, help="要量的产物（合成结果）")
    ap.add_argument("--person", required=True, help="人物图（掩码从它现推）")
    ap.add_argument("--erode", type=int, default=8, help="剪影向内腐蚀多少像素（默认 8）")
    ap.add_argument("--tol", type=int, default=90, help="没有 alpha 时判剪影的颜色阈值")
    ap.add_argument("--baseline", default="", help="纯合成的产物（判据的基线）")
    ap.add_argument("--max-drop", type=float, default=0.2, help="允许比基线低多少（默认 0.2 = 20%%）")
    args = ap.parse_args(argv)

    image = load_rgb(args.image)
    height, width = image.shape[:2]
    person_rgb, alpha, resized = load_person(args.person, (width, height))
    sil = silhouette(person_rgb, alpha, args.tol)
    mask = erode(sil, args.erode)

    got = measure(image, mask)
    if got is None:
        print(
            json.dumps(
                {
                    "error": "INTERIOR_EMPTY",
                    "detail": f"erode={args.erode} 之后内部一个像素都不剩（剪影占比 {float(sil.mean()):.4f}）",
                    "image": args.image,
                }
            )
        )
        return 2

    out = {
        "image": args.image,
        "person": args.person,
        "width": width,
        "height": height,
        "erode": args.erode,
        "kernel": LAPLACIAN_KERNEL,
        "personResized": resized,
        "maskSource": "alpha" if alpha is not None else "color",
        "silhouetteRatio": round(float(sil.mean()), 5),
        "maskRatio": round(float(mask.mean()), 5),
    }
    out.update(got)

    if args.baseline:
        base_rgb = load_rgb(args.baseline)
        if base_rgb.shape[:2] != (height, width):
            print(
                json.dumps(
                    {
                        "error": "BASELINE_SIZE_MISMATCH",
                        "detail": f"基线 {base_rgb.shape[1]}x{base_rgb.shape[0]} != 产物 {width}x{height}",
                        "image": args.image,
                        "baseline": args.baseline,
                    }
                )
            )
            return 2
        base = measure(base_rgb, mask)
        if base is None:
            print(json.dumps({"error": "BASELINE_INTERIOR_EMPTY", "baseline": args.baseline}))
            return 2
        ratio = got["laplacianVariance"] / base["laplacianVariance"] if base["laplacianVariance"] else 0.0
        out["baseline"] = {"path": args.baseline, **base}
        out["ratio"] = round(float(ratio), 4)
        out["maxDrop"] = args.max_drop
        out["pass"] = bool(ratio >= 1.0 - args.max_drop)

    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
