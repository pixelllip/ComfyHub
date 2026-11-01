---
name: moss-image
description: 用 MOSS-Image（OpenMOSS 5B 文生图 / 原生布局控制）写提示词并在 ComfyUI 出图。含官方提示词规则（描述成品、不写指令、逐字引号文字、Layout JSON 单套 schema：草稿 abs/xyxy → 编译 abs_centralized/yxyx）、SFT / Base 与 int8 / bf16 的取舍、官方与 ComfyUI 模板两套采样参数、SLG 第 22 层、官方四份 ComfyUI 模板（t2i / layout × SFT / Base）的用法与 TextGenerate 摘线法。
whenToUse: 用户提到 MOSS / MOSS-Image / moss_image / mossImage / 布局生图 / Layout JSON / 原生排版 / 中文文字渲染，要写 MOSS 提示词，要调 MOSS 的步数 / CFG / 分辨率 / SLG，问"MOSS 在 ComfyUI 里怎么配 / 怎么出图 / 这份 moss 工作流怎么用"，或者要用 moss 文生图 / 布局工作流出图时。
version: 3
---

# MOSS-Image：提示词 + ComfyUI 出图

## 0. 出处与边界（**按官方材料整理**）

- 官方仓库：<https://github.com/OpenMOSS/MOSS-Image>（README / USERGUIDE / comfyui README）
- **官方提示词 skill**：`skills/moss-image-prompt-enhancer/`（`SKILL.md` + `references/layout.md` +
  `scripts/compile_prompt.py`）—— **提示词腔调与 Layout JSON schema 的判据就是这三份**，
  和本 skill 冲突时以它为准。
- 官方项目页：<https://openmoss.ai/MOSS-Image/>
- ComfyUI 官方教程：<https://docs.comfy.org/zh/tutorials/image/moss-image/moss-image>
- 官方模板：ComfyUI 模板库里的四个 MOSS-Image 变体（见 §5）——**仓库不随包带模板**，
  出图起点就是官方模板，或用户自己另存的那份。

> **一句话**：MOSS-Image 是**文生图 / 布局引导文生图**模型，**不是图生图 / 参考图编辑模型**。
> 官方明确说：模板里的 `ref_image` 只喂给提示词扩写用的 LLM，**模型本身不支持 ref_image**。
> 用户要"按这张图改 / 换背景保人物"时，**不要拿 MOSS 工作流硬做**——换 Krea 2 / Anima / Flux Kontext
> 那条线（见 skill `krea-2`、`inpaint-composite`、`img2img-reference`）。

## 1. 模型与文件（缺一样就加载失败）

| 角色 | 文件 | 目录 |
| --- | --- | --- |
| 扩散模型（SFT，推荐） | `moss_image_sft_int8_convrot.safetensors`（或 bf16） | `models\diffusion_models\` |
| 扩散模型（Base，LoRA / 微调用） | `moss_image_base_int8_convrot.safetensors`（或 bf16） | 同上 |
| 文本编码器（Qwen3.5 9B） | `qwen3.5_9b_w4a8_gptq.safetensors` | `models\text_encoders\` |
| VAE（FLUX.2 small） | `flux2-vae_small.safetensors` | `models\vae\` |

- `CLIPLoader.type` **必须是 `moss_image`**（不是 `qwen_image`，也不是 `stable_diffusion`）。
  漏了它会出现"底模对了、文本按别的模型解"这种**不报错但结果不对**。
- SFT 是**指令微调**版：文字分组、排版更整洁，日常出图用它。
  Base 没有后训练 / CFG 蒸馏，是社区 LoRA 的起点；同一张图换加载器就能跑。
- 分辨率：**宽高必须是 16 的倍数**；推荐总像素 **1M / 1.5M / 2M**，可外推到约 0.25M~4M。
  模板默认 1024×1024（1MP）。改分辨率会改构图，不是"同一张图放大"。
- 提示词长度可到约 **4K token**；英文训练数据最多，**优先英文**，中文文字也能渲染。

## 2. 提示词怎么写（官方规则，照做）

1. **描述成品，不要写请求**。模型学的是 caption：
   ✅ `A minimalist travel poster with a cream background...`
   ❌ `帮我做一张极简旅行海报`
2. **保留用户说的一切**：主体、数量、动作、关系、媒介、风格、情绪、颜色、排除项；
   没说的用具体视觉选择补全（场景、构图、视角、光线、材质、色板、媒介）。
3. **只写看得见的东西**：不要声音、气味、背景故事、人物意图。
4. **画面里的文字逐字引号**：`a sign that says "OPEN 24 HOURS"`。
   保留用户的语言、拼写、大小写、标点、换行；**不要翻译**。
5. **排版位置用文字说清**：left / right / top / bottom / foreground / background；
   不要写坐标（要坐标就用 Layout JSON，见 §4）。
6. **质量词分模式，别把两套腔调混在一条 prompt 里**：
   - 写 **caption**（默认 / 扩写器的那条路）：官方 prompt-enhancer skill 明说 **Avoid redundant
     quality tags** —— `masterpiece, best quality, 8k, highly detailed` 这类堆叠没有意义。
   - 写 **tag 串**（Base 关掉扩写器时官方支持）：官方推荐顺序是
     **quality → score → meta → year → character → series → artist → general**，
     例：`best quality, highres, hatsune miku, vocaloid, @foo bar, 1girl, blue hair`
     （见 USERGUIDE "Tag 推荐顺序"）。也就是说 quality 在 tag 串里是**该放最前**的，别因为上一条就把它删光。
   - 负面提示词保持短，官方推荐就是模板默认的
     `nsfw, worst quality, bad anatomy, watermark, jpeg artifact`。
7. **换行要自己控**：自动换行可能不在你想要的位置；要精确换行就在文字里写真实换行或 `\n`。
8. **英文为主**；用户点名中文/日文文字时，按原文逐字保留，不要转拼音或英文。

## 3. 参数：官方档 vs ComfyUI 模板档，**别混着用**

| | 官方 Python / Gradio | ComfyUI 官方模板 |
| --- | --- | --- |
| steps | **≥ 50**（官方口径：≥50 步时效果最佳） | 25 |
| cfg | 3.0（推荐区间 **3.0–6.0**） | 2.5 |
| sampler | Euler（flow-matching） | `er_sde` |
| scheduler | 默认 | `simple` |
| shift | 5 | 5（模型默认） |
| 备注 | 效果基准，慢 | 速度折中（模板档） |

- **CFG 别照抄 2.5**：官方区间是 3.0–6.0（Base 没做 CFG 蒸馏）；长 prompt 用 3.0，
  **短 prompt 要适当调高**。2.5 是 ComfyUI 模板为提速选的，不是官方推荐值。
- **"官方效果"不是 50 步这个数字本身**，而是"≥50 步 + cfg 3 + Euler"这一组；只把步数拉到 50
  而 cfg 还是 1，出来的是另一种东西。
- 想快还有**社区 8 步 DMD 蒸馏 LoRA**（lodestones 的 `MOSS-Image-SFT-8-steps-dmd`）：
  挂上它把 steps/cfg 调成 **8 / 1**、sampler 仍用 `euler`。**那是社区档，不是官方档**，
  别和官方 50/3 混着说。

- **SLG（Skip Layer Guidance）**：官方是**负分支跳过第 22 层**（24 / 26 也可试）。
  模板里是 `SkipLayerGuidanceDiTSimple`（`single_layers="22"`，`start=0` / `end=0.5`）。
  关掉可能更"真实"，但细节会塌；动漫风格有时会被 SLG 过度锐化。
- 用户没要求就**别动** steps / cfg / sampler / 分辨率：工作流里的默认值是调过的。
  要"更接近官方效果"才建议 50 步 / cfg 3；要快就换 8 步 DMD 档。
- 显存不够：降分辨率、换 bf16/int8、或让用户在 ComfyUI 里开 Sequential / CPU offload。
- 训 LoRA 时**冻结文本编码器**（官方建议）。

## 4. 布局引导（Layout JSON）—— **只有一套 schema，没有"社区格式"**

判据是官方 `skills/moss-image-prompt-enhancer/references/layout.md` 与
`scripts/compile_prompt.py` 的校验（后者会**逐字段拒绝**）。它有两个形态，职责不同：

| | **草稿形态（你写这个）** | **编译形态（模型只吃这个）** |
| --- | --- | --- |
| 顶层 | 必须同时有 `width` / `height` | **不写** `width` / `height` |
| `bbox` 值 | `[x_min, y_min, x_max, y_max]` | `[y_min, x_min, y_max, x_max]` |
| 坐标系 | `"bbox_coord_space": "abs"`（左上原点、像素） | `"abs_centralized"`（画布中心原点） |
| 顺序 | `"bbox_format": "xyxy"` | `"yxyx"` |
| 形态 | 可读的多行 JSON | **一行紧凑 JSON** |

> 官方原话：**这些中心坐标的算术不要模型自己算**（"The model should never calculate these
> values itself"）。要么跑 `compile_prompt.py`，要么用 ComfyUI 的 `MOSS-Image Layout Prompt`
> 节点（它做的就是同一件事）；实在要手推就照 §4.2 的公式，并用官方那组数自检。

### 4.1 字段（**多一个键就会被拒**）

```json
{
  "width": 1536,
  "height": 1024,
  "bbox_coord_space": "abs",
  "bbox_format": "xyxy",
  "summary": {
    "meta": ["poster"],
    "canvas": "single",
    "caption": "整体场景与风格。"
  },
  "items": [
    {"label": "object", "caption": "一只象牙白陶瓷杯，三刻面角度，柔光。", "bbox": [176, 224, 800, 864]},
    {"label": "text", "caption": "深绿大字中文标题，无底板。", "text": "把日常，过成诗。", "bbox": [928, 272, 1408, 448]}
  ]
}
```

- `width` / `height`：正整数、**16 的倍数**，且**必须等于最终出图尺寸**；改画布要重排 box，
  不能只改这两个数（"Do not rescale boxes merely by changing these fields"）。
- `bbox_coord_space` 只能是 `"abs"`，`bbox_format` 只能是 `"xyxy"`（草稿）。带别的值直接报错。
- `summary` 是**对象**，不是字符串：`caption` 必填；`meta` / `canvas`
  （`single` / `multi_panel` / `collage`）/ `palette` 可选。
- `items` 是**扁平数组**，每项只有这些键：`label`（∈ `object` / `character` / `text`）、
  `caption`（必填，写外观）、`bbox`（**四个数的数组**）、`text`（`label="text"` 时**必填**，
  逐字保留原文，换行写成 JSON 的 `\n`）、`tags` / `palette`（可选）。
- `bbox` 必须在 `0..width` / `0..height` 内，且 `x1<x2`、`y1<y2`；越界是**拒绝**，不是静默裁剪。
- **非法键一个都不许有**：`type`、`x_min`/`y_min`/`x_max`/`y_max`、`children`/`parent`/`id`、
  scene schema 的 `elements`、归一化坐标、百分比、编辑器编号 —— 官方编译器遇到未知键**直接报错**
  （"rejects unrecognized fields rather than silently discarding creative intent"）。
  `type` **不是** `label` 的别名，`x_min…` **也不是** `bbox` 的别名：官方解析器
  （`compile_prompt.py` / `MOSS-Image Layout Prompt` 节点）认出这种键就报错；但**提交链路不做这层校验**，
  直接塞进 `positivePrompt` 不会有任何提示 —— 错误是**静默**的。
- 位置是**全局画布坐标**（连"画在物体上的文字"也用全局坐标）；多个 item 可以共框 / 重叠。

### 4.2 编译（草稿 → 模型输入）

```bash
python3 <skill>/scripts/compile_prompt.py draft.json --output prompt.txt
```

手推的话照这四步（与 `centered_box()` 逐字对齐）：

1. 小数边缘**向外取整**：`x_min`/`y_min` 向下取整（floor），`x_max`/`y_max` 向上取整（ceil）。
2. 再减中心：`x - width // 2`、`y - height // 2`（注意：x 减**宽**的一半，y 减**高**的一半）。
3. 顺序换成 `[y_min, x_min, y_max, x_max]`。
4. 输出**一行**紧凑 JSON，删掉 `width`/`height`，`bbox_coord_space="abs_centralized"`、
   `bbox_format="yxyx"`，`summary` / `label` / `caption` / `text` 原样保留。

自检（官方文档给的那组数）：1536×1024 上茶杯 `[176, 224, 800, 864]` → `[-288, -592, 352, 32]`。
对不上就是减错了边（`768 = 1536/2`、`512 = 1024/2`）。

### 4.3 ⚠ 别人给的 layout 工作流，提示词往往是错的

常见错法（本 skill 的上一版也这么教过）：外面套一层 Markdown 的 `json` 代码围栏、
`summary` 写成字符串、每个 item 用 `type`（`background` / `ui_element` / `text_block`）+
四个 `x_min`/`y_min`/`x_max`/`y_max` 键，既没有 `bbox_coord_space` / `bbox_format`，
坐标还越出画布 —— **按 §4.1/§4.2 重写**，否则"布局引导"名存实亡
（官方解析器认不出这段，而提交链路不校验，所以错得静默）。

> 上一版把它叫"另一种（社区）写法"、还写"用哪份工作流就照哪份的字段" ——
> **这是错的**：MOSS 只有一套 schema，照那份写 = 没写布局。

### 4.4 用法

- 把**编译形态**的一行 JSON 放进 `positivePrompt`，或直接覆盖正面 `CLIPTextEncode` 的 `text`
  （前提：它没有被连线）。
- 画布尺寸要等于**实际出图尺寸**（16 的倍数；`768×1368` 不合法，1368 不是 16 的倍数），
  别让 box 坐标系和出图尺寸两套。
- 布局只是"更强的位置约束"，不是像素级排版保证；文字仍可能被模型重排。
  要求逐字正确时，把文字放进 `text` 字段，`caption` 只写外观与字体/材质。

## 5. 工作流：用 ComfyUI 官方模板（**仓库里不带模板**）

MOSS-Image 的标准工作流由 **ComfyUI 官方模板库**提供（`Template` → 搜 `MOSS-Image`），四个变体：
`MOSS-Image: Text to Image` / `MOSS-Image Base: Text to Image` /
`MOSS-Image: Layout-Guided Text to Image` / `MOSS-Image Base: Layout-Guided Text to Image`。
需要 ComfyUI **0.42.0+**；四个模板跑同一个**子图**，加载器 / 采样器 / 画布尺寸都在子图里。

- **t2i 模板自带提示词增强器**（子图节点上的 `enable_prompt_upsampler`，默认 **true**）：
  短请求先被改写成完整 caption 再进模型，实际送进去的那段在 `Preview Any` 里可见。
  要"原样送我的 prompt"就把它关掉；增强器用的 system prompt 来自官方 prompt-enhancer skill。
- **layout 模板**用 `Create Bounding Boxes` 画框 + `MOSS-Image Layout Prompt` 编译成 §4 的 JSON：
  `enable_prompt_enhancer` **默认关**（打开会改写文本，可能把框丢掉）；框按绘制顺序输出，
  标成 text 的框渲染 `text`，其余是 `object`；人物框要在 `character_boxes` 里填预览里的编号。
- **节点 id 一律从 `apiGraph` 现读**（`comfy_load_workflow(includeGraph=true)`），
  不要背 id，也不要照抄别人的节点号。
- 用户要用**自己另存的工作流**时以那份为起点：先看清接线与实时参数，
  再 `comfy_diagnose_workflow` 看有没有悬挂线 / 旁路节点 / 缺口。
- **不要**从零拼节点图，也**不要**指望仓库里带一份现成模板（已删除）。

提交链照 `comfy-workflow-guide`：`comfy_load_workflow` → `comfy_check_inputs` →
`comfy_fit_workflow`（缺 `moss_image_*` / `qwen3.5_9b_*` / `flux2-vae_small` 时照它给的说明让用户下，
**绝不代下**）→ `comfy_submit`。模板里的 `TextGenerate` 按 §6 摘线。

## 6. TextGenerate：**你自己服从它的 system prompt，别让本地 LLM 扩写**

**纪律**：工作流里带 `TextGenerate` 时，尽量摘掉那条接线，
让 AI 读它的 `system_prompt`，**自己生成最终提示词**，再丢进正面条件编码。

### 6.1 三步（t2i 与 layout 通用）

1. `comfy_load_workflow(path=…, includeGraph=true)`，在 `apiGraph` 里找：
   - `TextGenerate` 节点（官方模板里在子图中，id 现读）；
   - 它的 `system_prompt` 来源（常见是 `PrimitiveStringMultiline`）；
   - 正面 `CLIPTextEncode` 以及它的 `text` 现在接在哪。
2. **按那段 system prompt 自己写**：
   - t2i：按 §2 写（**别设字数预算** —— 官方 enhancer 明确不要 arbitrary word budgets，
     描述到够为止；`80~200 词` 这类硬指标是错的）；
   - layout：按 §4.1 写草稿，再按 §4.2 编译成**一行**紧凑 JSON（`abs_centralized` / `yxyx`，
     删掉 `width`/`height`）。
3. `comfy_submit(promptId=…, positivePrompt="<你写好的最终提示词>")`。
   后端会把它写进正面 `CLIPTextEncode.text`，并把 `TextGenerate` 那一支**从提交图里摘掉**
   （含 `TextGenerate` / `PreviewAny` / `ComfySwitchNode` 与只喂它的上游）——
   节点不再存在，既不校验也不执行（只断线不够：ComfyUI 校验的是整张 prompt）。
   返回里看 `positivePromptApplied` 与 `textGenerateBypassed` 确认摘了哪几个节点。

### 6.2 不要做的事

- ❌ 把用户原话直接丢给 `TextGenerate` 让它扩写（慢、随机、可能把中文写成英文标签）。
- ❌ 覆盖一个**被连线**的正面 `text`（后端会拒绝：值是连线数组；就算硬改也无效）。
- ❌ 为了让 `positivePrompt` "看起来生效"去改用户的原始文件；`comfy_submit` 只改**这次提交的图**。
- ❌ 说"已经按你的要求扩写了"却没有 `positivePromptApplied` / `mediaIds` 证据。

## 7. 找哪份工作流 / 怎么提交

1. `comfy_list_workflows(query="moss")` 或 `comfy_find_workflow(query="moss")`。
2. 按用途选官方模板的变体：纯文生图 → `MOSS-Image: Text to Image`（Base 就用 Base 那份）；
   要精确布局 → `Layout-Guided Text to Image`。用户已经存过自己的 moss 工作流时，以他那份为起点。
   用户说"改图 / 参考图 / 换背景" → **不要选 MOSS**，换 Krea 2 / Anima / inpaint（§0）。
3. `comfy_load_workflow(path=…, includeGraph=true)` → `comfy_check_inputs` → `comfy_fit_workflow`
   （缺 `moss_image_*` / `qwen3.5_9b_*` / `flux2-vae_small` 时照 `comfy_fit_workflow` 的说明让用户下，
   **绝不代下**）。
4. `comfy_submit(promptId=…, positivePrompt=…, overrides={…})`。
5. 只认返回的 `mediaIds`；没有就说没有。

## 8. 常见故障（按现象查）

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 改了正面文本框没变化 | 它的 `text` 被连线覆盖（`Preview Any` / 开关节点那条链） | 改上游那个字符串节点的值，或用 `positivePrompt` 摘线 |
| 提示词被扩写成英文长文 | 增强器开着（`enable_prompt_upsampler` / `TextGenerate` 在跑） | 关掉增强器；或 `positivePrompt` 直接写最终稿 |
| "layout/edit 工作流"没用参考图 | `LoadImage` 只接旁路节点 | MOSS 本来就没有图片输入；要 i2i 换工作流 |
| **布局不生效 / JSON 被当散文读** | item 用了 `type`/`x_min…`、`summary` 写成字符串、外面套着 Markdown 代码围栏、坐标越界、或没编译成 `abs_centralized`/`yxyx` | 按 §4.1 重写、§4.2 编译；"模型不报错"不代表格式对 |
| 文字糊 / 缺笔画 / 乱码 | 分辨率太低 / 文字太长 / 没放 `text` 字段 | 提高分辨率、缩短文字、Layout 里逐字放 `text` |
| 画面过锐 / 动漫过曝 | SLG 第 22 层 | 把 SLG 节点旁路，或换 24/26 层 |
| 加载器报 `type` 不认 | `CLIPLoader.type` 不是 `moss_image` | 改成 `moss_image`；ComfyUI 要 **0.42.0+**（native 支持从 master 提交 `e087d8f6` 起，**0.41.3 不含**） |
| 模型加载失败 | 文件不在实例可见的模型根 | `comfy_fit_workflow` / `comfy_env_report`；让用户重启实例，别下载替代品 |
| 0.01 秒空跑 | 纯前端节点 / 版本不兼容节点 | 见 skill `comfy-workflow-audit`；用 `comfy_load_workflow` 原文件重读 |

## 9. 与其它 skill 的分工

- 工作流总则（怎么判断用途、怎么复制后再改、怎么给用户选）：`comfy-workflow-guide`。
- 提示词写法 / 参数细节：本 skill。
- 接线体检 / 空跑 / 白跑：`comfy-workflow-audit`。
- 换底模 / LoRA / 配方：`workflow-recipe`。
- 真正图生图 / 局部重绘：`img2img-reference`、`inpaint-composite`、`krea-2`。
