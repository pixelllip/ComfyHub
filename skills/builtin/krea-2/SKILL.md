---
name: krea-2
description: 写 Krea 2（K2）生图提示词，并用本机 ComfyUI 跑 Krea 2 RAW / Turbo（官方推荐参数：RAW 52 步 cfg 3.5 ≤1K，Turbo 8 步 cfg 0 mu 1.15 1K~2K）。含 Krea 官方 prompting 规则、提示词扩写纪律、风格参考（Srefs）与风格 LoRA 用法，以及本机两份现成模板（`comfyhub_t2i_krea2.json` 文生图 / `comfyhub_i2i_krea2.json` 参考图编辑）的节点结构与可覆盖键。正文按 Krea 官方材料整理（krea-ai/krea-2、krea-ai/skills、ComfyUI 官方 Krea-2 教程），模板部分是对用户两份真工作流的实测归纳。
whenToUse: 用户提到 Krea 2 / K2 / krea2，要写 Krea 2 提示词，要调 Krea 2 的步数 / CFG / mu / 分辨率 / 种子，要用 Krea 2 的风格参考或风格 LoRA，问"Krea 2 在 ComfyUI 里怎么配/怎么出图"，或者要用本机的 krea2 文生图 / 图生图模板出图时。
version: 2
---

## 版本记录

- **version 2（2026-10-17）**：新增 §6「本机的两份 krea2 模板」—— 用户当天从 ComfyUI 里存出来的
  `comfyhub_t2i_krea2.json` / `comfyhub_i2i_krea2.json` 的节点结构、可覆盖键与三个坑
  （广播只在前端生效、`Fast Bypasser` 是直通开关、`TextGenerate` 的动态下拉框）。
  起因：这两份工作流以前**转换就会缺一堆必填输入**，AI 只能即兴重写一份（然后跑偏）。


# Krea 2（K2）：提示词 + 本机 ComfyUI 出图

## 0. 这份 skill 的出处（**按官方材料整理，不是自己编的**）

内容来源（原文逐字版本放在本 skill 的 `references/`，采集日期 2026-09，见 `references/SOURCES.md`）：

- `krea-ai/krea-2` 官方仓库：`docs/prompting.md`（提示词指南）、`docs/expansion.txt`（官方扩写 system prompt）、
  `README.md`（RAW / Turbo 推荐参数）、`docs/safety.md`；
- `krea-ai/skills`（MIT）里 Krea 官方 skill 的 Krea 2 分册：`krea-generate/references/models/krea-2.md`；
- ComfyUI 官方教程《Krea-2 ComfyUI Workflow Example》（模型文件、8 步默认、风格参考管线、9 个风格 LoRA）；
- krea.ai 官方文档《Krea 2》（Medium / Large / Turbo 三档、Srefs、情绪板、生成滑杆、Creativity）。

> **注意**：本仓库的读路径白名单默认只有 `comfyui` 与 `storage`，所以**你自己读不到 `references/`**；
> 那几份原文是给人看的（用户想核对官方原话时，让他打开 `<根>\skills\builtin\krea-2\references\`）。
> 下面正文已经把"出图要用到的部分"写全了，不需要去读原文。

## 1. 提示词怎么写（官方规则，照做就行）

1. **用自然语言，不要堆 Danbooru tag**。官方明确推荐 natural language prompts ——
   K2 不是 SD1.5 那种 tag 模型，`masterpiece, best quality, 8k, ultra detailed` 这类咒语没有意义。
2. **长而具体最好**（官方："Long detailed prompts yield best results"），
   但**保真优先**：不许加用户没说的物体 / 人物 / 动物 / 道具，也不许把用户已经说清的细节改掉
   （用户自己写得很详细时，只做轻度润色与收尾）。
3. **一句话里把主体和它的属性绑在一起**，顺序建议：
   主体 + 属性 + 动作 → 场景 / 环境 → 构图 / 视角 / 镜头 → 光线 → 媒介 / 风格 → 质感 / 收尾。
4. **要渲染的文字加引号**：`a poster that says "OPEN 24 HOURS"`（官方明确推荐的做法）。
5. **保持用户点名的媒介**：说了 "photo of" 就出摄影、说 "illustration of" 就出插画、说 "3D render of" 就出 3D 渲染，
   **不许为了好画就换成别的媒介**。
6. **只输出提示词本体**：不要 planning 标签、不要 JSON、不要 markdown 列表、不要在正文里解释你选了哪种风格
   （官方 expansion.txt 第 3、6 条）。
7. **人物要有尊严**：默认人物穿好衣服；不写露骨 / 违法内容。开放权重下"部署方自己负责内容过滤"是官方许可的要求。
8. **负面提示词基本没用**：Turbo 是 `cfg=0`，根本没有负分支；RAW 的负分支官方默认是空字符串。
   想排除东西就把它写进正面描述（"a plain white background" 而不是 negative "no texture"）。

官方 `expansion.txt` 的九条规则就是上面这套的 system prompt 版本；要扩写时按它办。

## 2. 参数：RAW 与 Turbo（官方推荐值）

| | **Krea 2 RAW**（`oss_raw`，基座） | **Krea 2 Turbo**（`oss_turbo`，蒸馏） |
| --- | --- | --- |
| 步数 steps | **52** | **8** |
| CFG | **3.5** | **0**（关闭 CFG） |
| mu（timestep shift） | 按分辨率插值（`y1=0.5` @最小分辨率，`y2=1.15` @最大分辨率） | **钉死 1.15** |
| 分辨率 | 训练到 **1K**（别超） | **1K ~ 2K** |
| 用途 | 多样性 / 可塑性最强，**用来训 LoRA、微调** | **日常出图**，快且质量高 |

- 官方的用法一句话：**TRAIN on RAW, RUN on Turbo** —— RAW 上训出来的 LoRA 直接能用在 Turbo 上。
  用户要"多个变体、要探索风格"→ RAW；用户要"快点出图 / 迭代提示词"→ Turbo。
- **分辨率必须是 16 的倍数**（官方采样器会把非整数倍向上补齐；在 ComfyUI 里请直接给合法值，
  1K≈1024、2K≈2048；用户没说就沿用工作流里现有的分辨率，别擅自改）。
- 同一 seed + 同一提示词 + 同一参数才可复现；**改分辨率会改变画面构成**，不是"同一张图放大"。
- 官方云端还有 Intensity / Complexity / Movement 滑杆与 Creativity（raw / low / medium / high）——
  那是 **krea.ai 网页 / API 独有**的，本机 ComfyUI 里没有对应输入（见 §3.3）。

## 3. 在本机 ComfyUI 上怎么跑（我们的工具契约）

### 3.1 找图 → 只覆盖真实存在的输入

1. `comfy_find_workflow(query="krea", includeGraph=true)` —— 在 API 节点图里认三样东西：
   提示词节点（`CLIPTextEncode`，官方模板里是 "Text String (User Prompt)" 那个子图输入）、
   采样器的 `steps` / `cfg` / `seed`、分辨率来源（`EmptyLatentImage` 的 `width`/`height`，
   或模板里的 ResolutionSelector）。
2. `comfy_submit(promptId=…, overrides={"<节点id>.text": "…", "<节点id>.steps": 8, "<节点id>.cfg": 0.0})`
   - 键一律 `节点id.输入名`；**按原值类型写**（数字就是数字，别写成字符串）。
   - 字段不存在 / 是连线数组时后端会**明确拒绝**：这时**如实告诉用户"这个工作流没有这个输入"**，
     不要换个名字乱试到"看起来成功了"。
   - 用户没要求就别动 steps / cfg / seed / 分辨率 —— 工作流里的默认值就是它调好的。
3. 提交后**只认返回的 `mediaIds`**：没拿到就说没拿到，不许说"已经生成了"。
   超时会记 `timeout`，如实转述，不许假装完成也不许假装失败。

### 3.2 用用户的图做风格参考（Srefs）

- 本机走 ComfyUI 官方那条专用管线：`krea2_turbo_int8_convrot` + `krea2_style_reference` LoRA +
  `LoadImage` 参考图（模板名 "Krea-2 Style Reference"，可支持 1~2 张参考图）。
- 用户发来的图要先投放：`comfy_use_attachment(attachmentId=…)` 拿到它在 ComfyUI `input/` 里的真实文件名，
  再覆盖 `LoadImage` 的 `image` 输入 —— 详细三步与纪律见内置 skill **`img2img-reference`**（不要自己拼文件名）。
- 参考图强度、参考图张数：**只覆盖工作流里真实存在的输入**，没有就说没有。

### 3.3 云端独有能力：**不要假装本机能设**

| Krea 云端（krea.ai / API） | 本机 ComfyUI |
| --- | --- |
| Srefs 每张参考图的强度滑杆 | 取决于工作流有没有这个输入，没有就直说 |
| 情绪板 Moodboards（最多 1 个，strength 默认 0.23） | **没有** |
| Intensity / Complexity / Movement 滑杆 | **没有** |
| Creativity 模式（raw / low / medium / high，提示词扩写档位） | **没有**（模板里的 `prompt_enhance` 只是 LLM 扩写开关） |

要等价效果：把风格 / 色彩 / 质感**写进提示词**，或者换一个风格 LoRA。**不许编造字段名**。

### 3.4 风格 LoRA（官方随 Krea 2 发布，放在 `ComfyUI/models/loras/`）

触发词加进提示词，强度官方推荐 **1.0**：

| LoRA | 触发词（trigger word） |
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

### 3.5 用户问"Krea 2 在 ComfyUI 里怎么配"

官方（Comfy-Org 优化版）要三样文件，缺一样就会报节点 / 模型加载失败：

```
ComfyUI/models/diffusion_models/ krea2_turbo_fp8_scaled.safetensors   # Turbo，多数人推荐
ComfyUI/models/text_encoders/    qwen3vl_4b_fp8_scaled.safetensors    # Qwen3VL-4B 文本编码器
ComfyUI/models/vae/              qwen_image_vae.safetensors
# 风格参考另需：
ComfyUI/models/diffusion_models/ krea2_turbo_int8_convrot.safetensors
ComfyUI/models/loras/            krea2_style_reference.safetensors
```

还有 BF16 / NVFP4 / MXFP8 等变体（显存够用才上）。ComfyUI 要够新，模板在
模板库（Template Library）里搜 "Krea-2" / "Krea-2 Style Reference" 就有。

## 4. 纪律（与 `img2img-reference` 同一条底线）

- 提示词是提示词、参数是参数：**别把 steps / cfg / LoRA 名写进提示词正文**。
- 参数覆盖失败、附件投放失败、产物没回来 —— 一律**如实说**，不许假装成功。
- 用户没说要改参数就别改；**"我调了参数"不是"结果变好了"的证据**，要看 `mediaIds` 对应的产物。
- 拿不准工作流结构时先 `comfy_find_workflow(..., includeGraph=true)` 看清楚，再动手。

## 5. 官方原文（本目录 `references/`，给人核对用）

| 文件 | 内容 |
| --- | --- |
| `references/official-skill-krea-2.md` | Krea 官方 skill（`krea-ai/skills`）里的 Krea 2 分册原文，MIT |
| `references/official-prompting.md` | Krea 2 官方提示词指南（`docs/prompting.md`）原文 |
| `references/official-expansion-system-prompt.txt` | 官方扩写用的 system prompt（`docs/expansion.txt`）原文 |
| `references/official-comfyui-krea-2.md` | ComfyUI 官方 Krea-2 教程要点（模型 / 分辨率 / LoRA / 风格参考） |
| `references/SOURCES.md` | 上面每一份的 URL、采集日期、版本与许可 |

## 6. 本机的两份 krea2 模板（**要用 krea2 出图就先用它们，别现写**）

两份都在 `<根>\comfyui\workflows\`（仓库里也有一份同样的副本），是用户 2026-10-17
从 ComfyUI 里存出来的真工作流（界面格式）：

| 模板 | 干什么 | 结构（API 图节点数） |
| --- | --- | --- |
| `comfyhub_t2i_krea2.json` | **文生图**（Turbo 档：8 步 / euler / 896×1344） | 15 |
| `comfyhub_i2i_krea2.json` | **参考图编辑 / i2i**（LoadImage + 控制分支 + 提示词扩写） | 27 |

**怎么用**（三步，全是现成工具）：

1. `comfy_load_workflow(path="<根>\\comfyui\\workflows\\comfyhub_t2i_krea2.json", includeGraph=true)`
   —— 它会照 ComfyUI 前端的做法把前端节点等价改写掉（**必须开着 `emulateFrontend`，默认就是开的**），
   返回 `promptId` 与转换后的 `apiGraph`；**可覆盖键就从那份 `apiGraph` 里读**（`节点id.输入名`）。
2. `comfy_fit_workflow(promptId=…)` 看本机模型齐不齐（缺哪个文件、要不要换）。
3. `comfy_submit(promptId=…, overrides={"<节点id>.text": "…", …})`。

### 6.1 t2i 那张图的骨架（节点 id 就是模板里的 id）

```
28 CheckpointLoaderSimple(krea2Turbo…Int8) ─┐
30 CLIPLoader(type=krea2) ──────────────────┤→ 32 Lora Loader Stack(rgthree) ─┐
29 VAELoader(qwen_image_vae) ───────────────┘                                  │
                                   36 Anything Everywhere（广播 MODEL/CLIP/VAE）
                                                                               ↓
31 EmptyLatentImage(896×1344) → 33 Krea2TurboReferenceSigmaSchedulerFromLatent(8 步) ─┐
34 CLIPTextEncode(提示词) ──────────────────────────────────────────────────────────┤
42 RandomNoise(种子) → 40 SamplerCustomAdvanced ← 37 KSamplerSelect(euler) ←────────┘
                                    41 BasicGuider ← 34
40 → 38 VAEDecode → 39 SaveImage
```

| 想改什么 | 覆盖键 | 备注 |
| --- | --- | --- |
| 正面提示词 | `34.text` | Krea 2 用自然语言（§1），不是 tag |
| 分辨率 | `31.width` / `31.height` | 16 的倍数；改了会改构图，不是放大 |
| 种子 | `42.noise_seed` | |
| 步数 / 去噪 | `33.steps` / `33.denoise` | Turbo 档 8 步；**别乱调 cfg**（这张图没有 cfg，Turbo 是 cfg-free 那一路） |
| 底模 / VAE / 文本编码器 | `28.ckpt_name` / `29.vae_name` / `30.clip_name` | 换之前先 `comfy_fit_workflow` |
| LoRA 与强度 | `32.lora_01` … `32.lora_04` / `32.strength_01` … | 名字里带 `krea2` 的才是这一族的 |
| 输出前缀 | `39.filename_prefix` | |

### 6.2 i2i 那张图多了什么

主链与 t2i 同款，多出来的三块（都在主链旁边，**互不相干，可以只开一块**）：

1. **参考图 → 控制分支**：`42 LoadImage` → `97 RMBG` / `102 DepthAnythingPreprocessor`
   → `103 FluxKontextImageScale` → `104 Krea2ControlImageEncode` → `105 Krea2ControlApply`
   （LoRA 在 `89 Krea2ControlLoRALoader`，默认 `depth-control-lora.safetensors` 0.8）。
   要换参考图：先 `comfy_use_media` / `comfy_use_attachment` 把图投进 ComfyUI 的 `input/`，
   再覆盖 `42.image`（**别自己拼文件名**，见 skill `img2img-reference`）。
2. **提示词扩写**：`45 PrimitiveStringMultiline`（扩写用的 system prompt）→ `43 TextGenerate`
   （本地 LLM 扩写）→ 结果进 `108 CLIPTextEncode`。想直接给提示词就覆盖 `108.text`，
   或者把 `43` 那一路关掉（`mode=4` 旁路 / 让界面关掉它）。
3. **显示节点**：`55 PreviewAny` / `106 PreviewImage` —— 转换时会被丢掉，不影响出图。

### 6.3 三个坑（**踩过，别再踩**）

| 坑 | 现象 | 正确做法 |
| --- | --- | --- |
| **`Anything Everywhere` 只在前端生效** | 它在 `/object_info` 里**查得到**（cg-use-everywhere 注册了个零输出的桩），可服务端拿到它什么都不产出 —— 它该填的 MODEL/CLIP/VAE 那几路会一直空着。表现是"图看着没问题，提交上去缺 5 个必填输入" | 提交链**必须**走 `comfy_load_workflow`（它默认 `emulateFrontend=true`，会按前端行为把同类型的值填进去）。**不许**把这份界面格式文件直接当 API 图提交 |
| **`Fast Bypasser (rgthree)` 是直通开关** | 它不在 `/object_info` 里；不处理的话 `105.model ← 节点 90（没有转换出来）`，整份工作流都进不来 | 同上：`comfy_load_workflow` 会把它当"1 进 1 出"的直通接过去（判据来自真实提交记录：前端发出去的确实是 `105.model = ["89", 0]`） |
| **`TextGenerate.sampling_mode` 是动态下拉框** | 它的子项名字带点（`sampling_mode.temperature`），把主项当插口会报"必填输入空着"、丢掉子项会静默退回默认值 | 别手工拼这几个键；转换器已经按 `/object_info` + `widgets_values_named` 处理好了 |

### 6.4 两份模板的体检结论（2026-10-17 实测，供对照）

- **t2i**：0 错误。三条提醒：① 广播节点是"零输出桩"（见上表）；② `42 RandomNoise` 的控件值多一个
  `control_after_generate`（界面版本与后端声明不一致，已忽略）；③ **`35 ComfyUI-Krea2T-Enhancer`
  的输出没有接出去** —— 它在图里等于没生效，别以为它在起作用（要它生效就在 ComfyUI 里把
  `35.model` 接到该接的地方，再另存一份）。
- **i2i**：0 错误。除了上面那几条，还有 `TextGenerate` 的可选插口（`video` / `audio`）与
  `RMBG.background_color` 空着 —— **那是可选的，不是缺口**。

> 判据怎么来的：`comfy_audit_workflow` 只审 API 图，看不到这些；界面里点「查看工作流 → 看蓝图」
> 会跑一遍**接线体检**（同一份判据），哪根线接错、哪个插口空着、为什么，都在那份报告里。

