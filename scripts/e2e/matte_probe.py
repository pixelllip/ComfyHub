"""
抠图（matting）质量探针 —— **不需要 ComfyUI 在跑**，只借它的 venv 与模型文件。

为什么要有它：总纲 §15.1 第 4 条要求"照老规矩量化，别只看图"。这里就是那把尺子 ——
同一张图、同一套指标，量"颜色近似"与"AI 软遮罩"两档到底差多少。

两个子命令：

  run     在一张图上跑**真的抠图模型**，落盘 alpha + 合成图，并打印指标。
          模型来自 ComfyUI 的 `models\\rembg\\`（BEN2_Base.pth / RMBG-1.4.pth，**已在盘上**）
          或 rembg 包自带的会话（isnet-anime 等，缺了会下到 `~/.rembg/models`）。

  measure 把目录里所有 `alpha-*.png`（可配 `person-*.png`）用**同一套指标**量一遍并出对照图
          —— 用来比"现状（颜色抠图）vs 提议（AI 软遮罩）"。

指标口径（三档都是**合成之后**的量，不是遮罩内部的量）：
  · 前景占比     α>127 的像素占比 —— 掉前景（"人身上一块块变透明"/发丝被吃）就是它变小
  · 软像素占比   α∈(12,243) 的像素占比 —— **硬剪影恒为 0**；软遮罩才有连续 alpha
  · 边界溢色     合到**平灰(128)**上，边界带里量到的绿分量超出 —— 底不带绿，
                 所以量到的绿只能来自人物像素自己（"外轮廓留一圈绿边"就是它）
  · 细碎连通区   α>127 的小连通区（细发丝）条数与面积 —— 被"吃掉"就是它变 0

⚠ 别用"偏绿像素个数"自证干净（历史翻车点）：一定要同时看**平灰底合成图**与 1:1 放大图。

用法（pwsh 包装会自己找 venv）：
  pwsh -File scripts\\matte-probe.ps1 run     -Subject <人物图> [-Models BEN2,isnet-anime]
  pwsh -File scripts\\matte-probe.ps1 measure -Out <目录>
"""
from __future__ import annotations

import argparse
import glob
import importlib.util
import json
import os
import sys
import time

import numpy as np
from PIL import Image

GRAY = 128.0


# ---------------------------------------------------------------- 基础设施

def load_module(name: str, path: str):
    """按文件路径加载（ComfyUI 的自定义节点不是可导入的包，没有相对导入也不方便 sys.path）。"""
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def green_excess(rgb: np.ndarray) -> np.ndarray:
    g = rgb.astype(np.int32)
    return np.clip(g[..., 1] - np.maximum(g[..., 0], g[..., 2]), 0, 255)


def connected_strands(alpha: np.ndarray, thresh: int = 127):
    """α>thresh 的连通区统计 —— 小连通区就是"细发丝 / 飘发"那一类结构。"""
    import cv2

    mask = (alpha > thresh).astype(np.uint8)
    count, _, stats, _ = cv2.connectedComponentsWithStats(mask, connectivity=8)
    areas = sorted((int(stats[i, cv2.CC_STAT_AREA]) for i in range(1, count)), reverse=True)
    if not areas:
        return 0, 0, 0
    tiny = [a for a in areas if a < 0.0005 * alpha.size]
    return len(areas), len(tiny), sum(tiny)


def metrics(label: str, alpha: np.ndarray, rgb: np.ndarray, seconds: float) -> dict:
    soft = (alpha > 12) & (alpha < 243)
    blobs, tiny_n, tiny_area = connected_strands(alpha)
    excess = green_excess(rgb)
    return {
        "model": label,
        "seconds": round(seconds, 2),
        "fgFrac": round(float((alpha > 127).mean()), 5),
        "softFrac": round(float(soft.mean()), 5),
        "spillMean": round(float(excess[soft].mean()), 2) if soft.any() else 0.0,
        "spillMax": int(excess[soft].max()) if soft.any() else 0,
        "blobs": blobs,
        "tinyBlobs": tiny_n,
        "tinyArea": tiny_area,
    }


def composite_over(rgb: np.ndarray, alpha: np.ndarray, bg: np.ndarray) -> np.ndarray:
    a = alpha.astype(np.float64)[..., None] / 255.0
    return (rgb.astype(np.float64) * a + bg * (1 - a)).round().clip(0, 255).astype(np.uint8)


def load_bg(path: str, w: int, h: int) -> np.ndarray:
    """把背景等比放大到覆盖人物尺寸再中心裁 —— 合成出来才有可比性。"""
    bg = Image.open(path).convert("RGB")
    scale = max(w / bg.width, h / bg.height)
    bg = bg.resize((max(1, int(bg.width * scale)), max(1, int(bg.height * scale))), Image.LANCZOS)
    left, top = (bg.width - w) // 2, (bg.height - h) // 2
    return np.asarray(bg.crop((left, top, left + w, top + h))).astype(np.float64)


def print_table(rows: list[dict]) -> None:
    print(f"\n{'模型':<20}{'耗时s':>7}{'前景占比':>10}{'软像素占比':>11}"
          f"{'溢色均值':>9}{'溢色max':>8}{'连通区':>8}{'细碎':>6}{'细碎面积':>9}")
    print("-" * 90)
    for r in rows:
        print(f"{r['model']:<20}{r['seconds']:>7.2f}{r['fgFrac']:>10.5f}{r['softFrac']:>11.5f}"
              f"{r['spillMean']:>9.2f}{r['spillMax']:>8}{r['blobs']:>8}{r['tinyBlobs']:>6}{r['tinyArea']:>9}")
    print("-" * 90)


# ---------------------------------------------------------------- run

def find_ben2(comfy_home: str) -> str | None:
    for name in ("BEN2_Base.pth",):
        path = os.path.join(comfy_home, "models", "rembg", name)
        if os.path.isfile(path):
            return path
    return None


def find_rmbg14(comfy_home: str) -> str | None:
    path = os.path.join(comfy_home, "models", "rembg", "RMBG-1.4.pth")
    return path if os.path.isfile(path) else None


def run(args) -> int:
    subject = Image.open(args.subject).convert("RGB")
    w, h = subject.size
    rgb = np.asarray(subject)
    os.makedirs(args.out, exist_ok=True)
    print(f"主体: {os.path.basename(args.subject)}  {w}x{h}   ComfyUI: {args.comfy_home}")

    import torch

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    wanted = [m.strip() for m in args.models.split(",") if m.strip()]
    rows: list[dict] = []

    if "BEN2" in wanted:
        model_path = find_ben2(args.comfy_home)
        if not model_path:
            print("跳过 BEN2：models\\rembg\\BEN2_Base.pth 不在盘上（easy imageRemBg 首次调用时会去下）")
        else:
            mod = load_module(
                "ben_model",
                os.path.join(args.comfy_home, "custom_nodes", "comfyui-easy-use", "py", "modules", "ben", "model.py"),
            )
            t0 = time.time()
            model = mod.BEN_Base().to(device).eval()
            model.loadcheckpoints(model_path)
            print(f"BEN2 加载 {time.time() - t0:.1f}s  设备={device}")
            for refine in ([False, True] if args.refine else [False]):
                label = "BEN2(refine)" if refine else "BEN2"
                t0 = time.time()
                mask, _ = model.inference(subject.convert("RGBA"), refine)
                dt = time.time() - t0
                alpha = np.asarray(mask.convert("L"))
                rows.append(metrics(label, alpha, rgb, dt))
                save_outputs(args.out, label, alpha, rgb, args.background)
            del model
            torch.cuda.empty_cache()

    if "RMBG-1.4" in wanted:
        model_path = find_rmbg14(args.comfy_home)
        if not model_path:
            print("跳过 RMBG-1.4：models\\rembg\\RMBG-1.4.pth 不在盘上")
        else:
            mod = load_module(
                "briaai_rembg",
                os.path.join(args.comfy_home, "custom_nodes", "comfyui-easy-use", "py", "modules", "briaai", "rembg.py"),
            )
            net = mod.BriaRMBG()
            net.load_state_dict(torch.load(model_path, map_location="cpu"))
            net.to(device).eval()
            t0 = time.time()
            inp = mod.preprocess_image(subject, [1024, 1024]).to(device)
            res = net(inp)
            alpha = np.asarray(Image.fromarray(mod.postprocess_image(res[0][0], (h, w))).convert("L"))
            dt = time.time() - t0
            rows.append(metrics("RMBG-1.4", alpha, rgb, dt))
            save_outputs(args.out, "RMBG-1.4", alpha, rgb, args.background)

    others = [m for m in wanted if m not in ("BEN2", "RMBG-1.4")]
    if others:
        from rembg import new_session

        for name in others:
            try:
                session = new_session(name)  # 缺模型会下到 ~/.rembg/models（要出网）
            except Exception as exc:  # noqa: BLE001
                print(f"跳过 {name}：取会话失败（{exc}）")
                continue
            t0 = time.time()
            out = session.predict(subject)
            # ⚠ rembg 2.0.84 的 predict 回的是**一个列表，元素是 L 模式的遮罩本体**（不是 RGBA 抠图）
            if isinstance(out, list):
                out = out[0]
            dt = time.time() - t0
            alpha = np.asarray(out.convert("L"))
            rows.append(metrics(name, alpha, rgb, dt))
            save_outputs(args.out, name, alpha, rgb, args.background)

    if not rows:
        print("一个模型都没跑成 —— 检查 --comfy-home 与 --models")
        return 1
    print_table(rows)
    with open(os.path.join(args.out, "metrics.json"), "w", encoding="utf-8") as fh:
        json.dump(rows, fh, ensure_ascii=False, indent=2)
    print(f"已写 {args.out}\\alpha-*.png / comp-*.png / metrics.json")
    return 0


def save_outputs(out_dir: str, label: str, alpha: np.ndarray, rgb: np.ndarray, background: str) -> None:
    safe = label.replace("(", "_").replace(")", "").replace(".", "")
    Image.fromarray(alpha).save(os.path.join(out_dir, f"alpha-{safe}.png"))
    if background:
        bg = load_bg(background, alpha.shape[1], alpha.shape[0])
        Image.fromarray(composite_over(rgb, alpha, bg)).save(os.path.join(out_dir, f"comp-{safe}.png"))
    # 平灰底：底不带绿，于是边界带上量到的绿只能来自人物自己
    gray = np.full((*alpha.shape, 3), GRAY)
    Image.fromarray(composite_over(rgb, alpha, gray)).save(os.path.join(out_dir, f"gray-{safe}.png"))


# ---------------------------------------------------------------- measure

def measure(args) -> int:
    alpha_files = sorted(glob.glob(os.path.join(args.out, "alpha-*.png")))
    if not alpha_files:
        print(f"{args.out} 里没有 alpha-*.png —— 先跑 `run`")
        return 1
    rows = []
    panels = []
    print(f"{'alpha 文件':<28}{'前景占比':>10}{'软像素占比':>11}{'平灰底边界溢色':>15}{'max':>6}{'带内像素':>9}")
    print("-" * 82)
    for path in alpha_files:
        name = os.path.splitext(os.path.basename(path))[0][len("alpha-"):]
        alpha = np.asarray(Image.open(path).convert("L"))
        # 人物像素：先认同名的 `person-<name>.png`（颜色抠图那条链会落这一份，因为它的反混/despill
        # 是**改像素**的），认不到就回落成原始主体图（AI 那条链不改像素）。
        person_path = os.path.join(args.out, f"person-{name}.png")
        if os.path.isfile(person_path):
            rgb = np.asarray(Image.open(person_path).convert("RGB"))
        elif args.subject and os.path.isfile(args.subject):
            rgb = np.asarray(Image.open(args.subject).convert("RGB"))
        else:
            print(f"{name:<28} 找不到人物像素文件，跳过（给 --subject 或放一份 person-{name}.png）")
            continue
        gray = np.full((*alpha.shape, 3), GRAY)
        comp = composite_over(rgb, alpha, gray)
        ex = green_excess(comp)
        edge = (alpha >= 12) & (alpha < 243)
        n = int(edge.sum())
        mean = float(ex[edge].mean()) if n else 0.0
        top = int(ex[edge].max()) if n else 0
        soft = float(edge.mean())
        print(f"{name:<28}{float((alpha > 127).mean()):>10.5f}{soft:>11.5f}{mean:>15.2f}{top:>6}{n:>9}")
        rows.append({"name": name, "fgFrac": float((alpha > 127).mean()), "softFrac": soft,
                     "grayEdgeSpillMean": mean, "grayEdgeSpillMax": top, "edgePixels": n})
        if args.comparison:
            bg = load_bg(args.comparison, alpha.shape[1], alpha.shape[0])
            panels.append((name, composite_over(rgb, alpha, bg)))
    print("-" * 82)
    if panels:
        cw = min(p[1].shape[1] for p in panels)
        ch = min(p[1].shape[0] for p in panels)
        sheet = Image.new("RGB", (cw * len(panels), ch), (0, 0, 0))
        for i, (_n, img) in enumerate(panels):
            sheet.paste(Image.fromarray(img[0:ch, 0:cw]), (i * cw, 0))
        target = os.path.join(args.out, "measure-composite.png")
        sheet.save(target)
        print("并排对照（左→右 = " + " | ".join(p[0] for p in panels) + f"）: {target}")
    with open(os.path.join(args.out, "measure.json"), "w", encoding="utf-8") as fh:
        json.dump(rows, fh, ensure_ascii=False, indent=2)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="抠图质量探针")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_run = sub.add_parser("run", help="在真图上跑抠图模型")
    p_run.add_argument("--subject", required=True)
    p_run.add_argument("--out", required=True)
    p_run.add_argument("--comfy-home", required=True)
    p_run.add_argument("--models", default="BEN2")
    p_run.add_argument("--refine", action="store_true", help="BEN2 同时跑 refine_foreground 那一档")
    p_run.add_argument("--background", default="", help="合成用的真背景（可选）")
    p_run.set_defaults(func=run)

    p_measure = sub.add_parser("measure", help="把目录里的 alpha-*.png 用同一套指标量一遍")
    p_measure.add_argument("--out", required=True)
    p_measure.add_argument("--subject", default="", help="人物像素的兜底来源")
    p_measure.add_argument("--comparison", default="", help="并排合成用的背景（可选）")
    p_measure.set_defaults(func=measure)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
