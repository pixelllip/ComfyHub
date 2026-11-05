---
name: comfy-workflow-guide
description: 任何"用工作流出图 / 这份工作流是干什么的 / 帮我按这个风格做 / 换个模型"的第一站。先识别工作流的真实用途（t2i / i2i / edit / inpaint / upscale / video / audio），把候选列给用户挑，再照着现有工作流跑；绝不凭空编一份新节点图。要改就复制副本再改。含 TextGenerate（本地 LLM 扩写 / 布局编译器）的摘线流程：AI 服从它的 system prompt 自己写最终提示词，用 `comfy_submit(positivePrompt=…)` 注入正面条件编码。
whenToUse: 用户让你"生成 / 出一张图 / 按这份工作流 / 用这个风格 / 换个模型 / 这份工作流怎么用 / 它是什么用途 / 帮我改一下工作流"，或者你正准备从零搭节点图、准备把用户原话丢给 TextGenerate 扩写时。**先读这份，再决定用哪个专门的 skill。**
version: 1
---

# 工作流第一站：先看懂，再选，再复制

## 0. 一条硬纪律（2026-10-21 定稿）

> **不许自己编工作流。**
> 用户的机器上已经有工作流（ComfyUI 的 `user\<用户>\workflows\*.json`、库里捕获过的运行、
> 配方库里的骨架）。你的第一动作是**找出现成的**，把用途讲清楚，让用户选；
> 只有用户明确说"就要一份全新的 / 现有都不合适"时，才允许在**副本**上改。

**为什么**：AI 从零写的节点图，节点 id、控件顺序、前端 only 节点、动态下拉框、
模型文件名几乎一定有一处不对；而 ComfyUI 的报错往往只说"缺输入"，用户要花几轮才排得出来。
现成工作流是**已经跑通过的事实**，比生成的"看起来等价"可靠得多。

## 1. 四步（顺序不要调换）

### 第 1 步：找现有工作流

按这个顺序，前一步没有再走下一步：

1. `comfy_find_workflow(query="krea2" / "moss" / "anima" …)` —— 先搜库，库里没有会自动带上
   用户 ComfyUI 已保存的 `localWorkflows`（带 `path` / `inLibrary`）。
2. `comfy_list_workflows(query=…)` —— 直接列用户 ComfyUI 的 `workflows` 目录；可按文件名过滤。
3. `comfy_list_recipes(task="t2i" / "i2i" / "edit" / "inpaint" …)` —— 已经学过的骨架；
   它给 `skeletonHash` / 模型槽位 / 参数旋钮，适合"换个底模，但骨架不变"。

拿到候选后**不要自己挑一个就开跑**：把 2~4 个候选（名字 + 路径 + 你判断的用途 + 一句适合什么）
用 `ask_user_question` 摆给用户。用户界面上也有「选择现有工作流」按钮（可点「看蓝图」），
如果他是从那里选的，路径会直接出现在消息里 —— 那就以那条路径为准。

### 第 2 步：判断它到底能干什么（**看接线，不看文件名**）

先 `comfy_load_workflow(path=…, includeGraph=true)`，然后按下面的判据逐条核对。
**判据的优先级：采样器的 `latent_image` 从哪来 > 条件从哪来 > 输出节点 > 节点类名 > 文件名。**

| 用途 | 硬判据（在 API 图里看） | 常见误判 |
| --- | --- | --- |
| **t2i** 文生图 | 采样器的 `latent_image` 来自 `EmptyLatentImage` / `EmptyFlux2LatentImage` 一类**空 latent**；没有图片进采样器 | 文件名带 "edit"、图里有个没接线的 `LoadImage` → 仍然是 t2i |
| **i2i** 图生图 | 采样器的 `latent_image` 来自 `VAEEncode`（或 `LoadImage → VAEEncode`）；或 `denoise < 1` 且起点是图片 | 有 `LoadImage` 但只接到旁路 / 显示节点 → 不是 i2i |
| **edit / r2i** 参考图编辑 | 参考图经 `Krea2ControlImageEncode` / `FluxKontext` / `ReferenceLatent` / ControlNet 等**接进采样器的条件或 latent** | 参考图只喂给 `TextGenerate`（LLM 看图写提示词）→ 模型本身没用图 |
| **inpaint** 局部重绘 | 有遮罩链：`LoadImageMask` / `ImageToMask` / `SetLatentNoiseMask` / `InpaintModelConditioning`，且接进采样 | 有遮罩节点但旁路（`mode=4`）→ 没有在重绘 |
| **upscale** 放大 | 没有采样器，只有放大模型 / `ImageUpscaleWithModel` 一类 | 有采样器 + 放大支路 → 仍按 t2i/i2i 报，放大只是后处理 |
| **video** 视频 | 输出节点是 `SaveVideo` / `VHS_VideoCombine`，或有视频 latent / 视频模型节点 | 视频模型也能只出一张图（`SaveImage`）→ 按输出事实判 |
| **audio** 音频 | 输出 / 节点类是音频那一族 | — |
| **unknown** | 空图 / 既不吃图也不出图 | **如实说 unknown**，不要硬塞一个用途 |

三条核对纪律：

1. **`mode=4`（旁路）的节点不算**：ComfyUI 里旁路节点不执行。`LoadImage` / `TextGenerate` /
   `PreviewAny` 是 `mode=4` 时，它们对结果没有贡献。
2. **连线 ≠ 控件值**：`CLIPTextEncode.text` 如果是一个 `["节点id", 槽位]` 数组，它就被上游覆盖了；
   你在它自己的 `widgets_values` 里看到的文字**不一定生效**。
3. **别拿标题 / 文件名当证据** —— 实际案例：一份名为 `moss image edit workflow.json` 的工作流其实是
   **layout t2i**（`LoadImage` 只接旁路节点，采样器没吃图）。把"它叫 edit"当结论，
   接下来所有建议都会错。

把结论写成人话给用户：

> 这份是 **t2i**（空 latent 直接采样），实际提示词来自节点 30；节点 34 那个大文本框被连线覆盖了，
> 改它不生效。它不需要参考图，也**不能**做"按这张图改"。

### 第 3 步：按现有工作流跑（只覆盖真实存在的输入）

1. `comfy_load_workflow(path=…, includeGraph=true)` → 拿 `promptId` 与 `apiGraph`
   （**可覆盖键从 `apiGraph` 里读**，不要凭记忆写 `34.text`）。
2. `comfy_check_inputs(promptId=…, graph=…)` 看还缺什么（参考图 / 遮罩 / 必填连线）。
3. `comfy_fit_workflow(graph=…)` 看用户机器上模型齐不齐；缺件按它给的说明让用户处理，
   **不要改文件名硬凑，也不要代下**。
4. `comfy_submit(promptId=…, overrides={"节点id.输入名": 值})`；只改用户要求改的，
   `seed` / 分辨率 / steps 没要求就不动。
5. 只认返回的 `mediaIds`；没有产物就说没有。

### 第 4 步：要改就改**副本**

- `comfy_load_workflow` 读进库**不会改原文件**；但要落成一份可再用的文件时：
  - 蓝图页「保存」→ `<根>\comfyui\workflows\`（后端白名单决定落点）；
  - 或 `write_file` 另存 `<原名>_copy.json`（只能写 `comfyui` 目录）。
- 命名带 `_copy` / 日期，**不要覆盖原文件**；改完在回复里说清"改了哪几根线 / 哪几个参数"。
- 用户说"别动我的工作流"时，`comfy_load_workflow(..., emulateFrontend=false)` 只报缺口，
  一个字都不改；改图只在这次 `comfy_submit` 的 `overrides` / `positivePrompt` 里做。

## 2. `TextGenerate`：**你**来扩写，不让本地 LLM 跑

很多现成工作流里有一个 `TextGenerate`（本地 LLM）：它读 `system_prompt` + 用户短句，
输出扩写后的提示词，再经 `ComfySwitchNode` / `Fast Bypasser` 接到 `CLIPTextEncode.text`。
用户要的是：**你服从那段 system prompt，自己写最终提示词，然后摘掉 TextGenerate 那一路。**

### 2.1 识别

在 `apiGraph` 里找：

- 节点类名含 `TextGenerate`（大小写不敏感）；
- 它的 `inputs.system_prompt` 接在哪（通常是 `PrimitiveStringMultiline.value`，
  也可能直接是字符串控件）；
- 它的 `inputs.prompt` 接在哪（用户原话 / 另一个文本框）；
- 它的输出 `generated_text` 最终接到哪个 `CLIPTextEncode.text`
  （中间可能隔着 `ComfySwitchNode` / `PreviewAny` / `Fast Bypasser`）。

### 2.2 做

1. 把 `system_prompt` 的正文读出来（`read_file` 或 `comfy_get_run` 的 `raw` 里都有）。
2. **你自己按它写**：
   - 它是"把想法写成 caption"→ 你写一段 80~200 词的成品描述；
   - 它是"把需求编译成 Layout JSON"→ 你写 JSON，字段照它给的模板；
   - 它是"风格扩写"→ 你按它的规则扩写，保留用户原意。
3. `comfy_submit(promptId=…, positivePrompt="<最终稿>", overrides={…})`。
   - 后端会把最终稿写进**正面** `CLIPTextEncode.text`，并把 `TextGenerate` 这一支
     **从这次提交的图里摘掉**（节点 + 指向它的显示节点 + 只喂它的上游）。
     只断线不够：ComfyUI 校验的是整张 prompt，留在图里的 `TextGenerate` 仍会被查类名 /
     动态下拉框（用户没装那个节点包就整份报错）。
   - 返回里 `positivePromptApplied` / `textGenerateBypassed` 是证据；没有就别声称改好了。
4. 如果正面文本编码器本身是**字面量**（没有连线），也可以直接 `overrides={"<id>.text": …}`；
   `positivePrompt` 同样能用（它会重写那个控件）。
5. 如果工作流有多个正面编码器（ConditioningCombine 两条分支），
   `positivePrompt` 只改**离采样器最近的那一层**；更上游的风格 / 参考分支不动。
   不确定就先 `comfy_diagnose_workflow` 看清楚，再决定要不要用 `overrides` 单独改。

### 2.3 不要

- ❌ 把用户原话直接丢给 `TextGenerate` 去扩写（慢、随机、经常把中文写成英文标签）。
- ❌ 覆盖一个被连线覆盖的 `text`（后端会拒绝：值是连线数组）。
- ❌ 删掉用户工作流里的节点后说"已经改好"——`positivePrompt` 只在这次提交的图上摘，
  用户的原文件不动；要永久改就存副本并明确告诉他。

## 3. 给用户看蓝图

- 界面上：「选择现有工作流」→ 每行「看蓝图」；蓝图页有节点-连线、参数、接线体检、问题清单。
- AI 侧：`comfy_diagnose_workflow(path=… / promptId=… / graph=…)`，与界面**同一份判据**。
- 蓝图能回答的三件事：① 这份图长什么样；② 哪根线接错了 / 哪个必填空着；③ 哪些节点不参与出图。
- 拿不到 `/object_info` 时体检只做结构检查，**如实说覆盖面小一圈**，不要下"类型不对"的结论。

## 4. 该转给哪个 skill

| 用户要的 | 先读 |
| --- | --- |
| 判断用途 / 选工作流 / 复制后改 / TextGenerate 摘线 | **本 skill** |
| Krea 2 的提示词、参数、从**用户自己的 krea2 工作流**识别节点键 | `krea-2`（模板只作结构样例，不是起点） |
| MOSS-Image 的提示词、Layout JSON、两份模板 | `moss-image` |
| 换底模 / LoRA / VAE / 把骨架绑到新模型 | `workflow-recipe` |
| 跑不起来 / 0.01 秒空跑 / 白跑 / 参数档位 | `comfy-workflow-audit` |
| 把用户发来的图投放进 ComfyUI / 覆盖 LoadImage | `img2img-reference` |
| 局部重绘 / 换背景 / 贴人物 | `inpaint-composite` |
| 训 LoRA | `lora-training` |

## 5. 一页速查

```
找：comfy_find_workflow → comfy_list_workflows → comfy_list_recipes
看：comfy_load_workflow(includeGraph=true) / comfy_diagnose_workflow
判：latent 从哪来？条件从哪来？输出是什么？旁路了吗？文件名不算
选：2~4 个候选 + 用途 + 一句适合什么 → ask_user_question
跑：comfy_check_inputs → comfy_fit_workflow → comfy_submit(overrides)
扩：读 TextGenerate.system_prompt → 自己写 → comfy_submit(positivePrompt)
改：只改副本（蓝图保存 / write_file *_copy.json），原文件不动
报：改了哪几根线 / 哪几个参数；只认 mediaIds
```
