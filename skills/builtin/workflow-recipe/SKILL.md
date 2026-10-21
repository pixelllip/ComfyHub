---
name: workflow-recipe
description: 换（新）模型时怎么接上已有的工作流骨架——把一份具体工作流"学"成配方（任务 + 骨架指纹 + 模型槽位 + 参数旋钮），再"绑"到新模型上拿 comfy_submit 的 overrides。含四档绑定结论（exact / onDiskOnly / substitute / missing）的处置、整包 checkpoint 与分离式（unet+clip+vae）接不上时的两条路、以及"缺件配方不许开跑"的判据。工具是 comfy_learn_workflow / comfy_list_recipes / comfy_bind_recipe。
whenToUse: 用户说"出了个新模型，想用它做 XX"、"这条工作流能不能换个底模"、"这个模型该配哪条工作流"、"把这条工作流记下来下次还用"、"换个 LoRA / VAE / 文本编码器"，或者你正准备为某个新模型**从零搭一张图**时（先查有没有现成骨架）。也适用于"这份工作流是干什么的 / 它要哪些模型"这类问题。
version: 2
---

# 工作流配方：让"新模型"接上"旧骨架"

> **第一纪律（2026-10-21 用户拍板）**：**不许从零编工作流**。
> 先 `comfy_list_recipes` / `comfy_list_workflows` / `comfy_find_workflow` 找现成的；
> 没有现成的就"学一份"（`comfy_learn_workflow`），而不是现写节点图。
> 要改只在**副本**上改：`comfy_load_workflow` 读进库不动原文件；落盘用蓝图「保存」或
> `write_file` 另存 `*_copy.json`。原文件一个字节都不许改。
> 工作流里有 `TextGenerate` 时，由**你**服从它的 `system_prompt` 写终稿，
> 再 `comfy_submit(..., positivePrompt=…)` 把正提示词写进正面编码器并断开那一路
> （详见 skill `comfy-workflow-guide` §2）。

**一句话**：工作流拆成三层 —— **任务骨架（泛用）**、**模型槽位（特定）**、**参数档位（特定）**。
本机把学过的骨架记在 `workflow_recipes` 表里，换模型时只改槽位那几格，**不重搭图**。
设计与判据在 `docs/workflow-recipe-design.md`（这份 skill 只管"怎么用、怎么判断"）。

## 三层分别是什么（判据来自代码，不是猜的）

| 层 | 是什么 | 换模型时动不动 |
| --- | --- | --- |
| **骨架** | 执行相关节点的**类集合 + 类级连线**（丢节点 id、丢 Note/Reroute/Preview 这类界面节点）。指纹 = `skeletonHash`，**同一族 = 同一个值** | 不动（要动就是"改写骨架"，另一件事） |
| **模型槽位** | 从磁盘加载模型文件的输入：`CheckpointLoaderSimple.ckpt_name` / `UNETLoader.unet_name` / `CLIPLoader.clip_name`（**还有 `type`**）/ `VAELoader.vae_name` / `LoraLoader*.lora_name` / `ControlNetLoader` / `UpscaleModelLoader`… | **就是改这几格** |
| **参数档位** | 提示词 / 尺寸 / `steps` / `cfg` / `sampler_name` / `scheduler` / `denoise` / `seed` | 视模型而定（turbo 类通常 cfg≈1） |

> ⚠ `CLIPLoader.type` 属于**加载器参数**（`krea2` / `qwen_image` / `stable_diffusion`），
> 换模型族时**照样要改** —— 漏掉它会出现"底模换对了、文本编码器按老模型解"这种**不报错但结果不对**。

## 四步（顺序不要调换）

1. **先看有没有学过的**：`comfy_list_recipes(task="edit")`（任务按需换：`inpaint` / `i2i` / `t2i` / `video`…）。
   同一族会有相同的 `skeletonHash`。用 `q` 可以按槽位里的模型文件名搜（"这个模型我配过哪条工作流"）。
2. **没有就学一份**：`comfy_learn_workflow(path="…json")`（本机工作流文件，界面格式也行）/
   `promptId=…`（库里已有 API 图的提示词）/ `graph={…}`（API 节点图）。
   - 它回的是**配方正文**：任务、骨架指纹、槽位（覆盖键 + 原文件名）、旋钮、输出。
   - **被拒（`UNSUPPORTED_NODES`）时先读那几行**：它列出"转不出来被摘掉的节点类"与悬空的输入。
     真机上最常见的原因是**本机这个实例没装 / 没启用那个自定义节点包**
     （例：用户两份 Krea 2 编辑工作流要 `Krea2EditModelPatch` / `TextEncodeKrea2OstrisEdit`，
     而跑着的实例 `/object_info` 里一个都没有）——
     **不是工作流坏了**，也**不是**让你去改用户的工作流。处置：
     ① 让用户在 ComfyUI 里启停一次 / 换用装了那些节点的实例，再学一次；
     ② 在 ComfyUI 里「工作流菜单 → 导出（API）」，把那份 API 格式文件给我（零转换，一定学得会）；
     ③ 只想学个大概：`tolerateUnsupported=true` —— 配方会标 `complete=false` + 缺口清单。
3. **绑定**：`comfy_bind_recipe(recipeId=…, model="新模型.safetensors")`，拿 `overrides`。
   角色不用自己猜（按清单里该文件所在的目录对槽位）；不确定或同名多义（两条 LoRA）时用
   `role="loras"` 或 `overrideKey="71.lora_name"` 点明；要一起换文本编码器 / VAE 就用
   `extra={"text_encoders":"…","vae":"…"}`。
4. **提交**：`comfy_load_workflow(path="<配方里那条源文件>")` 拿 `promptId`
   （与配方的 `sourceRef = file:<sha256>` 是**同一把钥匙**），再
   `comfy_submit(promptId=…, overrides={…原来的槽位键换成新文件名…, 提示词 / seed 等})`。
   **配方本身不是可提交对象**（红线 R10：提交只有 `comfy_submit` 一条路）。
   带 `TextGenerate` 的骨架：**你**按它的 `system_prompt` 写终稿，走
   `comfy_submit(positivePrompt=…)`；不要覆盖被连线覆盖的 `text` 控件，也不要让本地 LLM 扩写。

## 四档绑定结论（每一档的处置都不一样）

| 结论 | 含义 | 你该做什么 |
| --- | --- | --- |
| `exact` | 本机有同名文件**且当前实例能加载** | ✅ 唯一能自动绑的一档，直接写进 overrides 提交 |
| `onDiskOnly` | 盘上有同名，但**这个实例枚举不到**（模型根没挂全） | ⛔ **别下载、也别改 overrides**：让用户重启 ComfyUI / 修模型根，再绑一次 |
| `substitute` | 本机只有**字面相似**的（相似≠架构兼容：`sd_xl_base` 与 `sd_xl_refiner` 分数很高却是两个模型） | ⛔ **先问用户**（那是他的模型），确认后才用 `force=true` 或让他自己指定 |
| `missing` | 本机确实没有 | ⛔ **绝不编文件名**：`find_model` / `civitai_search` 查它，再用 `ask_user_question` 给 2~4 个具体选项 |
| `unchecked` | 拿不到本机清单（ComfyUI 没跑 / 没找到 models 目录） | ⚠ 没验证过；确实要试就 `force=true`，提交前预检还会再拦一次 |
| `notInRecipe` | 这条配方里没有这个角色 / 这个键 | 看报告里列出的槽位清单，换 `role` / `overrideKey` 或换一条骨架 |
| `ambiguous` | 同角色有多个槽位（两条 LoRA） | 用 `overrideKey` 点明一个；**不要**把一个文件写给两个槽位 |
| `notAModel` | 名字是 ComfyUI 写的 sidecar（`*.metadata.json`） | 用同名的 `.safetensors` |

## 接不上时怎么判断（别硬接）

- **整包 checkpoint ↔ 分离式（unet + clip + vae）**：这不是改个文件名的事 —— 要**增删节点并重连线**。
  报 `notInRecipe` 且写明"要改写骨架"时，两条路：① 找这个模型**对应的形态**
  （要分离式的就找 unet / text encoder / vae 分开的版本）；② 换一条**形态对得上**的同任务骨架
  （`comfy_list_recipes(task=…)`）。
- **`complete=false` 的配方**：学的时候就缺件（缺节点类）。**先把缺的补齐再绑**，
  拿它开跑会得到一条看着对、其实缺链的图。
- **`roleGuessed=true` 的槽位**：那是 `/object_info` 现算出来的自定义加载器，覆盖键是准的，
  **目录名是代码猜的** —— 别拿目录名当结论去让人装东西。

## 沉淀（这是"自主学习"那一半）

跑通一次之后，把"这条骨架 + 这个模型 + 这组档位"如实说出来：
配方库里已经有这条骨架（`skeletonHash` 不变），下次同一个模型直接用同一条配方的槽位键，
**不必重新试**。要补充来源 / 用途就用 `comfy_learn_workflow(…, notes="…")` 覆盖同一条（幂等）。
用户说"以后都用这个模型做这类图"时，用 `remember` 记一条事实（一行一条）。
