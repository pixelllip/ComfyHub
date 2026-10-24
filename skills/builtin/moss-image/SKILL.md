---
name: moss-image
description: 用 MOSS-Image（OpenMOSS 5B 文生图 / 原生布局控制）写提示词并在本机 ComfyUI 出图。含官方提示词规则（描述成品、不写指令、逐字引号文字、Layout JSON）、SFT / Base 与 int8 / bf16 的取舍、官方与模板两套采样参数、SLG 第 22 层、以及本机两份真实工作流（`comfyhub_moss_t2i.json` 文生图、`comfyhub_moss_layout.json` 布局引导）的节点结构、可覆盖键和 TextGenerate 摘线法。
whenToUse: 用户提到 MOSS / MOSS-Image / moss_image / mossImage / 布局生图 / Layout JSON / 原生排版 / 中文文字渲染，要写 MOSS 提示词，要调 MOSS 的步数 / CFG / 分辨率 / SLG，问"MOSS 在 ComfyUI 里怎么配 / 怎么出图 / 这份 moss 工作流怎么用"，或者要用本机的 moss 文生图 / 布局工作流出图时。
version: 1
---

# MOSS-Image：提示词 + 本机 ComfyUI 出图

## 0. 出处与边界（**按官方材料 + 本机真实工作流整理**）

- 官方仓库：<https://github.com/OpenMOSS/MOSS-Image>（README / USERGUIDE / comfyui README）
- 官方项目页：<https://openmoss.ai/MOSS-Image/>
- ComfyUI 官方教程：<https://docs.comfy.org/zh/tutorials/image/moss-image/moss-image>
- 本机工作流：`<根>\comfyui\workflows\comfyhub_moss_t2i.json` 与 `comfyhub_moss_layout.json`
  （用户 2026-10 从 ComfyUI 存出来的真工作流；节点 id 就是下面写的那些）。

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
6. **不要质量咒语**：`masterpiece, best quality, 8k, highly detailed` 对 MOSS 没有意义，
   官方扩写提示词也明确禁止。负面提示词保持短，模板默认
   `nsfw, worst quality, bad anatomy, watermark, jpeg artifact`。
7. **换行要自己控**：自动换行可能不在你想要的位置；要精确换行就在文字里写真实换行或 `\n`。
8. **英文为主**；用户点名中文/日文文字时，按原文逐字保留，不要转拼音或英文。

## 3. 参数：三套档位，**别混着用**

| | 官方 Python / Gradio | ComfyUI 官方模板 | 本机 `comfyhub_moss_t2i.json` |
| --- | --- | --- | --- |
| steps | 50 | 25 | **8** |
| cfg | 3.0 | 2.5 | **1** |
| sampler | Euler（flow-matching） | `er_sde` | `euler` |
| scheduler | 默认 | `simple` | `simple` |
| shift | 5 | 5 | 5（模型默认） |
| 备注 | 效果基准，慢 | 速度折中 | SFT + 8-step DMD LoRA，最快 |

- **SLG（Skip Layer Guidance）**：官方跳过第 **22** 层（24 / 26 也可试）。
  模板里 `43 SkipLayerGuidanceDiTSimple` 的 `single_layers="22"`，`44 ComfySwitchNode.switch=true` 打开。
  关掉可能更"真实"，但细节会塌；动漫风格有时会被 SLG 过度锐化。
- 用户没要求就**别动** steps / cfg / sampler / 分辨率：工作流里的默认值是调过的。
  要"更接近官方效果"才建议 50 步 / cfg 3；要快就用模板 8 步档。
- 显存不够：降分辨率、换 bf16/int8、或让用户在 ComfyUI 里开 Sequential / CPU offload。
- 训 LoRA 时**冻结文本编码器**（官方建议）。

## 4. 布局引导（Layout JSON）

MOSS 能读一个 JSON 布局，指定每个物体 / 每行文字的位置。官方示例（**绝对中心坐标、yxyx**）：

```json
{
  "summary": {"meta": ["poster"], "canvas": "single", "caption": "..."},
  "items": [
    {"label": "text", "caption": "Large dark-blue uppercase title.", "text": "EXPLORE", "bbox": [-416, -384, -288, 384]},
    {"label": "object", "caption": "A flat blue mountain illustration.", "bbox": [-192, -288, 384, 288]}
  ],
  "bbox_coord_space": "abs_centralized",
  "bbox_format": "yxyx"
}
```

- 官方格式：`bbox = [y_min, x_min, y_max, x_max]`，相对画布中心，像素；
  `bbox_coord_space` / `bbox_format` 要一起写。
- 本机 `comfyhub_moss_layout.json` 用的是**另一种**（社区）写法：
  `type` / `caption` / `x_min,y_min,x_max,y_max`（左上原点），**没有** `bbox_format`。
  两套格式**不要混写**；用哪份工作流就照哪份的字段。
- 布局只是"更强的位置约束"，不是像素级排版保证；文字仍可能被模型重排。
  要求逐字正确时，把文字放进 `text` 字段，`caption` 只写外观。

## 5. 本机两份工作流（**要用 MOSS 就先用它们，别现写**）

都在 `<根>\comfyui\workflows\`（仓库里也有同样副本）。

### 5.1 `comfyhub_moss_t2i.json` —— 文生图（SFT + 8 步 DMD）

**它实际在跑什么**：`34 CLIPTextEncode(Positive Prompt)` 的 `text` 是**连线**（来自
`38 PreviewAny ← 39 ComfySwitchNode`），不是控件值。`39.switch=false` 时选 `30 Text (User Prompt)`；
`true` 时选 `32 TextGenerate`（本地 LLM 扩写）。所以：

- 你在 ComfyUI 里改 `34` 那个大文本框**不会生效**（它被连线覆盖了）——
  这是这份工作流最容易踩的坑。要改提示词：改 `30.value`，或让 `39.switch=false`。
- 默认 `39.switch=false`，`32 TextGenerate` 即使还在图里也不该被当成"正在扩写"。
- `31 Text (System prompt)` 是官方扩写 system prompt 的逐字版；`32` 的 `system_prompt` 接的就是它。

骨架（节点 id 就是模板里的 id）：

```
33 UNETLoader(moss_image_sft_int8_convrot) ┐
35 CLIPLoader(type=moss_image)             ├→ 43 SkipLayerGuidanceDiTSimple(22)
37 VAELoader(flux2-vae_small)              ┘        ↓ 44 ComfySwitchNode(switch=true)
                                                   ↓ 46 LoraLoaderModelOnly(8-step DMD, 1.0)
                                                   ↓ 48 LoraLoader(mode=4 旁路，rin_yuu)
30 PrimitiveStringMultiline(User Prompt) ┐
31 PrimitiveStringMultiline(System)      ├→ 32 TextGenerate ─┐
                                          └→ 39 ComfySwitchNode(switch=false)
                                                              ↓ 38 PreviewAny
                                                              ↓ 34 CLIPTextEncode(Positive)
19 ResolutionSelector → 40 EmptyFlux2LatentImage(1024×1024)
34 + 41(negative) + 40 → 36 KSampler(8 步/cfg1/euler/simple)
36 → 42 VAEDecode → 9 SaveImage(prefix=moss_image_t2i)
```

可覆盖键：

| 想改什么 | 键 | 备注 |
| --- | --- | --- |
| 正面提示词（推荐） | `30.value` | 默认档下它才是真正送进编码器的文字 |
| 正面提示词（绕开连线） | `positivePrompt`（`comfy_submit` 参数） | 直接写进 `34.text` 并断开 `38/39` 那一路，见 §6 |
| 分辨率 | `19.aspect_ratio` / `19.megapixels` / `19.multiple` | 或直接覆盖 `40.width` / `40.height`；16 的倍数 |
| 种子 | `36.seed` | |
| 步数 / CFG / 采样器 | `36.steps` / `36.cfg` / `36.sampler_name` / `36.scheduler` | 默认 8 / 1 / euler / simple；别和官方 50 / 3 混用 |
| 底模 | `33.unet_name` | 换 Base / bf16 前先 `comfy_fit_workflow` |
| 文本编码器 | `35.clip_name` + `35.type` | `type` 必须是 `moss_image` |
| VAE | `37.vae_name` | |
| 8-step DMD LoRA | `46.lora_name` / `46.strength_model` | 没有这个 LoRA 时把 `46` 旁路或换普通 LoRA |
| SLG | `43.single_layers` / `44.switch` | 官方第 22 层；关了细节可能塌 |
| 输出前缀 | `9.filename_prefix` | 抽卡时保留前缀、只换 seed |

### 5.2 `comfyhub_moss_layout.json` —— 布局引导文生图（**不是图生图**）

**先看清它到底在做什么**：

- `6 CLIPTextEncode(Positive Prompt)` 的 `text` 是**字面量 Layout JSON**（社区格式：`x_min/y_min/x_max/y_max`）。
- `17 LoadImage` 的输出**只接到 `19 TextGenerate`（mode=4 旁路）**，
  没有任何一条线进采样器 → **它没有用参考图**，名字叫 "edit" 但实际是 **layout t2i**。
- `18 Text String (System Prompt)` 是布局编译器提示词；`19 TextGenerate` 被旁路，
  所以默认不会跑本地 LLM。
- ⚠ **仓库里这份模板做过一次净化**：用户原始文件里 `6.text` 被一条来自
  `22 PreviewAny`（`mode=4` 旁路）的线覆盖着，转换时会留下"6.text ← 节点 22"的悬挂引用。
  模板已把那条线和节点 22 去掉，所以 `6.text` 现在就是真正生效的控件值。
  如果你用的是**用户原文件**而不是模板，先 `comfy_diagnose_workflow` 看有没有这条悬挂线；
  有就说明"改 6.text 不生效"，直接用 `positivePrompt` 写最终 Layout JSON。

用法：把用户需求写成**社区格式**的 Layout JSON，放进 `positivePrompt`（或覆盖 `6.text`；
模板里它没有被连线）。分辨率在 `2 ResolutionSelector` / `10 EmptyFlux2LatentImage`。

> ⚠ 这份模板的 `5 UNETLoader` 写的是 `mossImage_base_pruned_int8.safetensors`（用户本机的一份
> Base 变体）。如果你机器上只有官方的 `moss_image_base_int8_convrot.safetensors`，
> 先 `comfy_fit_workflow` 看清单，再覆盖 `5.unet_name` 成实际有的那一份；**别把文件名改一半**。

### 5.3 两份模板的体检结论（2026-10-21 实测）

- **t2i**：界面格式转换后 0 错误；`48 LoraLoader` 是 `mode=4`（旁路），`52 Fast Groups Bypasser`
  是前端开关（`comfy_load_workflow` 默认 `emulateFrontend=true` 会处理）。
- **layout**：0 错误；`18/19/22` 都是 `mode=4`，`17 LoadImage` 不参与出图 ——
  **别把"文件名带 edit"当成 i2i 的证据**（判据是接线，不是名字）。
- 两份都依赖 `TextGenerate.sampling_mode` 这种动态下拉框（子项键名带点）。
  别手工拼这些键；`comfy_load_workflow` 已按 `/object_info` + `widgets_values_named` 处理。

## 6. TextGenerate：**你自己服从它的 system prompt，别让本地 LLM 扩写**

用户 2026-10-21 的纪律：工作流里带 `TextGenerate` 时，尽量摘掉那条接线，
让 AI 读它的 `system_prompt`，**自己生成最终提示词**，再丢进正面条件编码。

### 6.1 三步（t2i 与 layout 通用）

1. `comfy_load_workflow(path=…, includeGraph=true)`，在 `apiGraph` 里找：
   - `TextGenerate` 节点（`32` 或 `19`）；
   - 它的 `system_prompt` 来源（`31.value` 或 `18.value`）；
   - 正面 `CLIPTextEncode`（`34` 或 `6`）以及它的 `text` 现在接在哪。
2. **按那段 system prompt 自己写**：
   - t2i：写成 80~200 词英文 caption（§2 的规则）；
   - layout：写成 Layout JSON（§4，注意这份工作流用 `x_min...` 社区格式）。
3. `comfy_submit(promptId=…, positivePrompt="<你写好的最终提示词>")`。
   后端会把它写进正面 `CLIPTextEncode.text`，并把 `TextGenerate` 那一支**从提交图里摘掉**
   （含 `TextGenerate` / `PreviewAny` / `ComfySwitchNode` 与只喂它的上游）——
   节点不再存在，既不校验也不执行（只断线不够：ComfyUI 校验的是整张 prompt）。
   返回里看 `positivePromptApplied` 与 `textGenerateBypassed` 确认摘了哪几个节点。

### 6.2 不要做的事

- ❌ 把用户原话直接丢给 `TextGenerate` 让它扩写（慢、随机、可能把中文写成英文标签）。
- ❌ 覆盖一个**被连线**的 `34.text` / `6.text`（后端会拒绝：值是连线数组；就算硬改也无效）。
- ❌ 为了让 `positivePrompt` "看起来生效"去改用户的原始文件；`comfy_submit` 只改**这次提交的图**。
- ❌ 说"已经按你的要求扩写了"却没有 `positivePromptApplied` / `mediaIds` 证据。

## 7. 找哪份工作流 / 怎么提交

1. `comfy_list_workflows(query="moss")` 或 `comfy_find_workflow(query="moss")`。
2. 按用途选：纯文生图 → `comfyhub_moss_t2i.json`；要精确布局 → `comfyhub_moss_layout.json`。
   用户说"改图 / 参考图 / 换背景" → **不要选 MOSS**，换 Krea 2 / Anima / inpaint（§0）。
3. `comfy_load_workflow(path=…, includeGraph=true)` → `comfy_check_inputs` → `comfy_fit_workflow`
   （缺 `moss_image_*` / `qwen3.5_9b_*` / `flux2-vae_small` 时照 `comfy_fit_workflow` 的说明让用户下，
   **绝不代下**）。
4. `comfy_submit(promptId=…, positivePrompt=…, overrides={"36.seed":…})`。
5. 只认返回的 `mediaIds`；没有就说没有。

## 8. 常见故障（按现象查）

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 改了 `34.text` 没变化 | `34.text` 被 `38/39` 的连线覆盖 | 改 `30.value`，或用 `positivePrompt` 断线 |
| 提示词被扩写成英文长文 | `39.switch=true` / `TextGenerate` 在跑 | 设 `39.switch=false`；或 `positivePrompt` 直接写最终稿 |
| "edit 工作流"没用参考图 | `LoadImage` 只接旁路节点 | 它本来就是 layout t2i；要 i2i 换工作流 |
| 文字糊 / 缺笔画 / 乱码 | 分辨率太低 / 文字太长 / 没放 `text` 字段 | 提高分辨率、缩短文字、Layout 里逐字放 `text` |
| 画面过锐 / 动漫过曝 | SLG 第 22 层 | 试 `44.switch=false` 或换 24/26 层 |
| 加载器报 `type` 不认 | `CLIPLoader.type` 不是 `moss_image` | 改成 `moss_image`；确认 ComfyUI ≥ 0.41.3 |
| 模型加载失败 | 文件不在实例可见的模型根 | `comfy_fit_workflow` / `comfy_env_report`；让用户重启实例，别下载替代品 |
| 0.01 秒空跑 | 纯前端节点 / 版本不兼容节点 | 见 skill `comfy-workflow-audit`；用 `comfy_load_workflow` 原文件重读 |

## 9. 与其它 skill 的分工

- 工作流总则（怎么判断用途、怎么复制后再改、怎么给用户选）：`comfy-workflow-guide`。
- 提示词写法 / 参数细节：本 skill。
- 接线体检 / 空跑 / 白跑：`comfy-workflow-audit`。
- 换底模 / LoRA / 配方：`workflow-recipe`。
- 真正图生图 / 局部重绘：`img2img-reference`、`inpaint-composite`、`krea-2`。
