# ComfyUI 官方 Krea-2 教程要点

> 来源：<https://docs.comfy.org/tutorials/image/krea/krea-2>（`.md` 版本：`/tutorials/image/krea/krea-2.md`）
> 采集日期：2026-09。下面是**要点摘录**（模型文件、默认参数、风格参考管线、风格 LoRA），
> 不是逐字全文；要看完整教程（含工作流下载、视频）请打开上面的链接。

## 两个变体

- **Krea 2 RAW**：基座，full-step（52 步），无蒸馏，多样、可塑，**用来微调 / 训 LoRA**。
- **Krea 2 Turbo**：8 步蒸馏，快且质量高，**日常出图用它**。RAW 上训的 LoRA 可直接用于 Turbo。
- 许可：Krea AI Community License（<https://www.krea.ai/krea-2-licensing>）。

## 官方工作流

| 工作流 | 说明 |
| --- | --- |
| Krea-2: Text to Image（`image_krea2_turbo_t2i`） | 文本生图；默认 **8 步**、开提示词增强、不带 LoRA |
| Krea-2 Int8: Image Style Reference（`image_krea2_turbo_int8_image_style_reference`） | 用 1~2 张参考图影响风格 / 氛围；专用 int8 模型 + 风格参考 LoRA |

模板在工作流模板库（Template Library）里搜 "Krea-2" / "Krea-2 Style Reference" 即可；
ComfyUI 要够新，缺节点多半是版本旧。

工作流里能调的东西（原文 "Workflow controls"）：

- Text String (User Prompt)：提示词
- `prompt_enhance` / `LLM_max_token`：LLM 提示词扩写开关与长度
- Width / Height：由 ResolutionSelector 控制（**Krea 2 支持 1K ~ 2K**；2K 就把 megapixels 设成 2.0）
- Seed
- `enable_lora?` / LoRA Strength / LoRA Trigger Word：风格 LoRA

## 模型文件（Comfy-Org/Krea-2 优化版）

```
📂 ComfyUI/
├── 📂 models/
│   ├── 📂 diffusion_models/
│   │   └── krea2_turbo_fp8_scaled.safetensors      # Turbo FP8，多数人推荐
│   ├── 📂 text_encoders/
│   │   └── qwen3vl_4b_fp8_scaled.safetensors       # Qwen3VL-4B 文本编码器
│   ├── 📂 vae/
│   │   └── qwen_image_vae.safetensors
│   └── 📂 loras/
│       └── krea2_softwatercolor.safetensors        # 以及其它风格 LoRA
```

其它变体：BF16 / NVFP4 / MXFP8（显存够用才上）。

风格参考工作流另外需要：

```
models/diffusion_models/krea2_turbo_int8_convrot.safetensors
models/loras/krea2_style_reference.safetensors
```

## 官方风格 LoRA（触发词要写进提示词，推荐强度 1.0）

| LoRA | Trigger Word |
| --- | --- |
| `krea2_darkbrush` | monochrome ink wash style |
| `krea2_dotmatrix` | monochrome stippling style |
| `krea2_kidsdrawing` | naive expressive sketch style |
| `krea2_neondrip` | textured abstract style |
| `krea2_rainywindow` | rainy window style |
| `krea2_retroanime` | purple retro anime style |
| `krea2_softwatercolor` | art deco watercolor style |
| `krea2_sunsetblur` | ethereal motion blur style |
| `krea2_vintagetarot` | vintage tarot style |
