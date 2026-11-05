---
name: krea-2
description: 写 Krea 2（K2）生图提示词，并用**用户自己在 ComfyUI 里存出来的 krea2 工作流**出图（官方推荐参数：RAW 52 步 cfg 3.5 ≤1K，Turbo 8 步 cfg 0 mu 1.15 1K~2K）。含 Krea 官方 prompting 规则、提示词扩写纪律、风格参考（Srefs）与风格 LoRA 用法，以及"先找用户工作流 → 读 apiGraph 认节点 → 只覆盖真实存在的输入 → 带 TextGenerate 时由 AI 服从其 system prompt 写终稿再 `positivePrompt` 摘线"的完整流程。**不依赖随包模板**：模板只作结构样例，出图起点必须是用户自己存出来的那一份。
whenToUse: 用户提到 Krea 2 / K2 / krea2，要写 Krea 2 提示词，要调 Krea 2 的步数 / CFG / mu / 分辨率 / 种子，要用 Krea 2 的风格参考或风格 LoRA，问"Krea 2 在 ComfyUI 里怎么配/怎么出图"，或者要用他自己 ComfyUI 里保存的 krea2 工作流出图时。
version: 5
---

## 版本记录

- **version 2（2026-10-17）**：新增两份 krea2 模板的节点结构、可覆盖键与三个坑
  （**2026-10-28 起本软件不再随包带模板，这两份已删除**；结构说明见 §6）。
- **version 3（2026-10-21）**：新增「工作流优先 / 副本 / TextGenerate 摘线」；
  修正"直接覆盖 `108.text`"的错误说法（它被 `116 PrimitiveString` 连线覆盖，覆盖会被拒），
  改为 `comfy_submit(positivePrompt=…)`。
- **version 4（2026-10-21）**：**不再把随包的两份模板当默认起点**。
  出图一律先找**用户自己从 ComfyUI 里存出来的 krea2 工作流**，按它的实际 `apiGraph` 识别节点；
  模板只作为结构样例，**不许拿它当"现成工作流"直接跑**。
- **version 5（2026-10-28）**：本软件**不再随包带模板**（两份 `comfyhub_*_krea2.json` 已删除，
  §6 的结构样例一并移除）。起点只有两个：**官方 Krea-2 模板**、**用户自己另存的那份**。

> **⚠ 唯一默认起点：用户自己的 krea2 工作流（或官方 Krea-2 模板）。**
> 本软件**没有**任何随包模板可拿。用户还没有自己的工作流时，教他：
> 在 ComfyUI 里打开官方 Krea-2 模板 → **另存为** `我的krea2_xxx.json`（放在
> `user\<用户>\workflows\`）→ 再以那份为起点；**不要**自己拼节点图。

# Krea 2（K2）：提示词 + ComfyUI 出图

## 0. 这份 skill 的出处（**按官方材料整理，不是自己编的**）

内容来源（原文逐字版本放在本 skill 的 `references/`，采集日期 2026-09，见 `references/SOURCES.md`）：

- `krea-ai/krea-2` 官方仓库：`docs/prompting.md`（提示词指南）、`docs/expansion.txt`（官方扩写 system prompt）、
  `README.md`（RAW / Turbo 推荐参数）、`docs/safety.md`；
- `krea-ai/skills`（MIT）里 Krea 官方 skill 的 Krea 2 分册：`krea-generate/references/models/krea-2.md`；
- ComfyUI 官方教程《Krea-2 ComfyUI Workflow Example》（模型文件、8 步默认、风格参考管线、9 个风格 LoRA）；
- krea.ai 官方文档《Krea 2》（Medium / Large / Turbo 三档、Srefs、情绪板、生成滑杆、Creativity）。

> **注意**：本软件的读路径白名单默认只有 `comfyui` 与 `storage`，所以**你自己读不到 `references/`**；
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
   （官方 expansion.txt 第 3、6 条）。交付时按系统提示第 24 条把这一段放进三反引号 `text` 围栏
   （围栏里仍是纯文本，不带任何标签），让用户能一键复制。
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

- 官方用法一句话：**TRAIN on RAW, RUN on Turbo** —— RAW 上训出来的 LoRA 直接能用在 Turbo 上。
  用户要"多个变体、要探索风格"→ RAW；要"快点出图 / 迭代提示词"→ Turbo。
- **分辨率必须是 16 的倍数**（官方采样器会把非整数倍向上补齐；在 ComfyUI 里请直接给合法值，
  1K≈1024、2K≈2048；用户没说就沿用工作流里现有的分辨率，别擅自改）。
- 同一 seed + 同一提示词 + 同一参数才可复现；**改分辨率会改变画面构成**，不是"同一张图放大"。
- 官方云端还有 Intensity / Complexity / Movement 滑杆与 Creativity（raw / low / medium / high）——
  那是 **krea.ai 网页 / API 独有**的，ComfyUI 里没有对应输入（见 §3.5）。

## 3. 在 ComfyUI 上怎么跑（**从用户的工作流开始**）

### 3.1 第一步：找用户自己保存的 krea2 工作流

按顺序：

1. `comfy_list_workflows(query="krea")` —— 列用户 ComfyUI 已保存的工作流文件；
2. `comfy_find_workflow(query="krea2")` —— 库里 / 磁盘上的文件一起搜；
3. 如果有多份，把**名字 + 路径 + 你判断的用途**（t2i / i2i / 参考图编辑 / 放大…）
   用 `ask_user_question` 让用户挑；**别替他默认选一份**。

**不要**：
- ❌ 自己拼一份"等价"的节点图；
- ❌ 用户说"随便"就挑一份名字带 krea 的——先打开图看接线，确认它真的是 krea2 出图。

用户还没有自己的 krea2 工作流时：让他在 ComfyUI 里打开官方 Krea-2 模板，**另存为**自己的工作流；
这份另存出来的文件才是起点。

### 3.2 第二步：读 `apiGraph`，按实际接线认节点

`comfy_load_workflow(path=…, includeGraph=true)`，然后**从返回的 `apiGraph` 里现读**下面几样，
**不要背节点 id**（换一份工作流 id 就全变了）：

| 要找什么 | 怎么认 |
| --- | --- |
| 正面提示词节点 | 顺着采样器的 `positive` / `BasicGuider.conditioning` 往上找 `CLIPTextEncode`；它的 `text` 可能是一个控件，也可能是连线 |
| 采样参数 | `KSampler` / `SamplerCustomAdvanced` 那条链上的 `steps` / `cfg` / `sampler_name` / `scheduler` / `seed` |
| 分辨率 | `EmptyLatentImage.width/height`，或 `ResolutionSelector` 的 `aspect_ratio` / `megapixels` / `multiple` |
| 模型 | `CheckpointLoaderSimple` / `UNETLoader` / `CLIPLoader(type=krea2)` / `VAELoader` / `LoraLoader*` |
| 参考图支路 | `LoadImage` → 控制预处理 / `Krea2ControlImageEncode` / ControlNet → 是否真的接进采样器 |
| 提示词扩写 | `TextGenerate` 及其 `system_prompt` 来源（常是 `PrimitiveStringMultiline`） |

**三个坑**（任何一份 krea2 工作流都可能遇到）：

1. **`Anything Everywhere` 只在前端生效**：它在 `/object_info` 里可能查得到（注册成零输出桩），
   服务端拿到它什么都不产出。提交链**必须**走 `comfy_load_workflow`（默认 `emulateFrontend=true`）。
2. **`Fast Bypasser (rgthree)` 是直通开关**：不在 `/object_info` 里；不处理会让下游指向一个"没转换出来"的节点。
   同样交给 `comfy_load_workflow` 处理，别手工拼。
3. **`TextGenerate.sampling_mode` 是动态下拉框**：子项键名带点（`sampling_mode.temperature`），
   别手工拼；转换器已按 `/object_info` + `widgets_values_named` 处理。

### 3.3 第三步：只覆盖真实存在的输入

```text
comfy_submit(promptId=…, overrides={"<节点id>.text": "…", "<节点id>.steps": 8, "<节点id>.cfg": 0.0})
```

- 键一律 `节点id.输入名`；**按原值类型写**（数字就是数字，别写成字符串）。
- 字段不存在 / 是连线数组时后端会**明确拒绝**：**如实告诉用户"这个工作流没有这个输入"**，
  不要换个名字乱试到"看起来成功了"。
- ⚠ **文本编码器的 `text` 是连线时**，`overrides` 改不动它：用
  `comfy_submit(positivePrompt="最终提示词")`（见 §7.3），不要硬写那个节点。
- 用户没要求就别动 steps / cfg / seed / 分辨率 —— 工作流里的默认值就是它调好的。

提交后**只认返回的 `mediaIds`**：没拿到就说没拿到，不许说"已经生成了"。
超时会记 `timeout`，如实转述，不许假装完成也不许假装失败。

### 3.4 用用户的图做风格参考（Srefs）

- 前提：**用户自己的工作流里真的有**风格参考那条管线（`krea2_style_reference` LoRA +
  `LoadImage`）。没有就如实说"这份工作流没有参考图输入"，不要替他加节点。
- 用户发来的图要先投放：`comfy_use_attachment(attachmentId=…)` 拿到它在 ComfyUI `input/` 里的真实文件名，
  再覆盖 `LoadImage.image` —— 详细三步与纪律见内置 skill **`img2img-reference`**（不要自己拼文件名）。
- 参考图强度、参考图张数：**只覆盖工作流里真实存在的输入**，没有就说没有。

### 3.5 云端独有能力：**不要假装 ComfyUI 能设**

| Krea 云端（krea.ai / API） | ComfyUI 本地 |
| --- | --- |
| Srefs 每张参考图的强度滑杆 | 取决于工作流有没有这个输入，没有就直说 |
| 情绪板 Moodboards（最多 1 个，strength 默认 0.23） | **没有** |
| Intensity / Complexity / Movement 滑杆 | **没有** |
| Creativity 模式（raw / low / medium / high，提示词扩写档位） | **没有**（工作流里的 LLM 扩写开关不等价） |

要等价效果：把风格 / 色彩 / 质感**写进提示词**，或者换一个风格 LoRA。**不许编造字段名**。

### 3.6 风格 LoRA（官方随 Krea 2 发布，放在 `ComfyUI/models/loras/`）

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

用户自己的工作流里没有 `LoraLoader` 时，**不要为了加风格 LoRA 改结构**；把风格写进提示词即可。

### 3.7 用户问"Krea 2 在 ComfyUI 里怎么配"

官方（Comfy-Org 优化版）要三样文件，缺一样就会报节点 / 模型加载失败：

```text
ComfyUI/models/diffusion_models/ krea2_turbo_fp8_scaled.safetensors   # Turbo，多数人推荐
ComfyUI/models/text_encoders/    qwen3vl_4b_fp8_scaled.safetensors    # Qwen3VL-4B 文本编码器
ComfyUI/models/vae/              qwen_image_vae.safetensors
# 风格参考另需：
ComfyUI/models/diffusion_models/ krea2_turbo_int8_convrot.safetensors
ComfyUI/models/loras/            krea2_style_reference.safetensors
```

还有 BF16 / NVFP4 / MXFP8 等变体（显存够用才上）。ComfyUI 要够新；
官方模板在 ComfyUI 的 Template Library 里搜 "Krea-2" / "Krea-2 Style Reference"。
**但下载/打开官方模板只是"造一份自己的工作流"的手段**，真正出图仍以用户另存出来的那份为起点。

## 4. 纪律（与 `img2img-reference` 同一条底线）

- 提示词是提示词、参数是参数：**别把 steps / cfg / LoRA 名写进提示词正文**。
- 参数覆盖失败、附件投放失败、产物没回来 —— 一律**如实说**，不许假装成功。
- 用户没说要改参数就别改；**"我调了参数"不是"结果变好了"的证据**，要看 `mediaIds` 对应的产物。
- 拿不准工作流结构时先 `comfy_load_workflow(..., includeGraph=true)` 看清楚，再动手。

## 5. 官方原文（本目录 `references/`，给人核对用）

| 文件 | 内容 |
| --- | --- |
| `references/official-skill-krea-2.md` | Krea 官方 skill（`krea-ai/skills`）里的 Krea 2 分册原文，MIT |
| `references/official-prompting.md` | Krea 2 官方提示词指南（`docs/prompting.md`）原文 |
| `references/official-expansion-system-prompt.txt` | 官方扩写用的 system prompt（`docs/expansion.txt`）原文 |
| `references/official-comfyui-krea-2.md` | ComfyUI 官方 Krea-2 教程要点（模型 / 分辨率 / LoRA / 风格参考） |
| `references/SOURCES.md` | 上面每一份的 URL、采集日期、版本与许可 |

## 6. 结构参考（**节点 id 一律 `apiGraph` 现读**）

> **2026-10-28：本软件不再随包带模板**，原来那两份 `comfyhub_t2i_krea2.json` /
> `comfyhub_i2i_krea2.json`（以及它们的骨架图、可覆盖键表、体检结论）已整体删除。
> 现在只用两个来源：**ComfyUI 模板库里的官方 Krea-2 模板**，或**用户自己另存的那份**。
>
> 读任何一份 krea2 工作流时都按 §3.2 的表逐项从 `apiGraph` 里认：正面 `CLIPTextEncode`、
> 采样参数、分辨率、模型加载器、参考图支路、`TextGenerate`。**不要背节点 id** ——
> 换一份工作流，id、节点数、甚至主链都会变。
>
> 结构上仍然会遇到的三个坑（与具体文件无关）：`Anything Everywhere` 只在前端生效、
> `Fast Bypasser (rgthree)` 是直通开关、`TextGenerate.sampling_mode` 是动态下拉框 ——
> 处理方式见 §3.2 与 `comfy-workflow-guide`。

## 7. 工作流优先 / 副本 / TextGenerate 摘线（2026-10-21 定稿）

### 7.1 不许从零编，也不拿随包模板顶替

- 要 krea2 出图时，**只用用户自己存出来的 krea2 工作流（或官方 Krea-2 模板）**作为起点；
  先 `comfy_list_workflows(query="krea")` / `comfy_find_workflow(query="krea2")` 找，
  把候选与用途列给用户选。**不要**自己拼节点图 —— 本软件里**没有**可回退的模板。
- 用户说"换新模型 / 换个底模"时，先看 `comfy_list_recipes(task=…)`；
  要改骨架也只在**副本**上改（蓝图保存 / `write_file` 存 `*_copy.json`），原文件不动。

### 7.2 先讲清用途（看接线，不看文件名）

- 采样器的 `latent_image` 来自 `EmptyLatentImage` 一类空 latent → **t2i**；
- `LoadImage` 经 VAEEncode / 控制分支真的接进采样器 → **i2i / 参考图编辑**；
- 有遮罩链且接进采样 → **inpaint**；
- 判断"是不是 i2i"要看 `LoadImage` 的输出有没有真的进采样器；只看文件名 / 标题会错。
- 用户给的工作流如果有 `TextGenerate`，还要看它的输出最终接到哪个正面 `CLIPTextEncode`。

### 7.3 TextGenerate 摘线（通用流程）

只要用户的工作流里有 `TextGenerate`（本地 LLM 扩写 / 布局编译器）：

1. `comfy_load_workflow(path=用户选中的那份, includeGraph=true)`；
2. 在 `apiGraph` 里找 `TextGenerate`、它的 `system_prompt` 来源、以及正面 `CLIPTextEncode`；
   读那段 system prompt，**你自己按它扩写**用户的需求（保真优先、不改主体 / 动作 / 颜色 /
   空间关系、不堆质量词）；
3. `comfy_submit(promptId=…, positivePrompt="<你写好的终稿>", overrides={…参考图、seed 等})`。
   后端会把终稿写进正面 `CLIPTextEncode.text`，并把 `TextGenerate` 这一支
   **从提交图里摘掉**（含开关 / `PreviewAny` 与只喂它的上游）—— 节点不再存在，
   既不校验也不执行（只断线不够：ComfyUI 校验的是整张 prompt）。
4. 返回 `positivePromptApplied` / `textGenerateBypassed` 是证据；没有就如实说没改成。

> 那一支可能是 `TextGenerate → 开关 → PreviewAny → 正面 CLIPTextEncode` 这样串起来的；
> **具体 id 一律以实际 `apiGraph` 为准**，不要照抄任何示例。

**不要**：
- ❌ 覆盖一个被连线覆盖的 `text`（后端会拒绝）；
- ❌ 覆盖 `TextGenerate` 的 `system_prompt` 来源或它的控件值然后声称"已经按你的提示词生成"
  （那只是换了 system prompt，仍然让本地 LLM 扩写）；
- ❌ 把用户原话直接丢给 `TextGenerate`（它会把中文改写成英文标签，且每次温度不同）。

### 7.4 跑之前

`comfy_check_inputs` 看参考图 / 遮罩缺不缺；`comfy_fit_workflow` 看
`krea2_turbo_*` / `qwen3vl_4b_*` / `qwen_image_vae` / 控制 LoRA 在不在。
缺件让用户下（**绝不代下**），不要改文件名硬凑。
