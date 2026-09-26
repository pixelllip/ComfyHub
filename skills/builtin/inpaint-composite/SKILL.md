---
name: inpaint-composite
description: 图像编辑的局部重绘工作流——保背景换人物 / 换背景保人物 / 把先生成好的人物贴进某张图。用 comfy_make_mask 生成遮罩、comfy_use_media 投放画廊里的底图与人物图，跑项目自带的两条 inpaint 工作流（comfyui/workflows/comfyhub_inpaint_anima.json / comfyhub_inpaint_krea2.json），两条都靠"白=重绘区"的遮罩决定保哪里。
whenToUse: 用户说"保持背景不变，换个人 / 换个动作 / 换身衣服"、"背景换掉但人物不变"、"把这张图里的人换成…"、"先生成人物再放进去"、局部重绘 / inpaint / 遮罩 / 贴人 / 合成人物时。
version: 1
---

# 图像编辑：局部重绘（保背景换人物 / 换背景保人物）

**一句话**：局部重绘就是"**一张底图 + 一张遮罩 + 一段只描述遮罩区的提示词**"。
遮罩里的白 = 要重新生成的地方，黑 = 原样保留。项目自带两条现成工作流，
**两种需求用同一份图，只是遮罩反过来**：

| 需求 | `region`（人物那块） | `invert` | `20.image`（人物图） | `17.denoise` |
| --- | --- | --- | --- | --- |
| **保背景换人物**（换人 / 换动作 / 换装） | 人物所在位置 | `false` | 底图自己 或 阶段一的人物图 | `1.0`（纯重绘）/ `0.4~0.6`（贴人物图） |
| **换背景保人物** | 人物所在位置 | **`true`** | 底图自己 | `0.85~1.0` |
| 整张重画（等价图生图） | `full` | `false` | 底图自己 | `0.4~0.7` |
| **两阶段**：先生成人物再换上去 | 人物该出现的位置 | `false` | **阶段一产物的 filename** | `0.4~0.6` |

## 三步（顺序不要调换）

1. **底图进 ComfyUI 的 input 目录**（`LoadImage` 只能读那里面的文件）：
   - 画廊里的产物 → `comfy_use_media(mediaId=…)`；
   - 用户随消息发来的图 → `comfy_use_attachment(attachmentId=…)`；
   - **两者返回里一定要拿 `filename` 与 `width`/`height`**（后面遮罩和 ImageScale 都照它填）。
2. **生成遮罩**：`comfy_make_mask(baseFilename="<上一步的 filename>", region=…, invert=…, feather=…)`
   - `region` 支持 `full` / `left` / `right` / `top` / `bottom` / `center` / `person`
     （人物竖框）或归一化的 `"x,y,w,h"`（例 `"0.30,0.05,0.42,0.90"`）；
   - 结果里的 `approximate=true` 表示"框是我估的"——**要如实转述**，并请用户给一组新坐标（或放大框）；
   - 边缘要自然就 `feather=8~24`；把人物贴进新背景时几乎总要羽化。
3. **提交**（工作流是项目自带的，路径固定，**别自己编节点**）：
   ```
   comfy_load_workflow(path="<项目根>\\comfyui\\workflows\\comfyhub_inpaint_anima.json", includeGraph=true)
     → 拿 promptId（Anima 底模：Danbooru tag 角色；摄影/写实用 comfyhub_inpaint_krea2.json）

   comfy_submit(promptId=…, overrides={
     "10.image":  "<底图 filename>",
     "20.image":  "<底图 filename 或 人物图 filename>",
     "12.image":  "<遮罩 filename>",
     "11.width": W, "11.height": H,      // 与底图**必须一致**
     "21.width": W, "21.height": H,
     "15.text":  "<只写遮罩区里应该出现什么>",
     "17.seed":  <换一个新的数字>,
     "17.denoise": 1.0                   // 贴人物图时改 0.4~0.6
   })
   ```
   Krea2 那份的提示词节点是 `13.text`、采样器是 `15.*`（**自然语言**，别堆 tag）；
   Anima 那份是 `15.text` + `17.*`（**Danbooru tag**，写角色前先按系统提示第 18 条查三处）。

## 两阶段：先生成人物，再换上去（用户要的正是这个）

1. **阶段一（t2i）**：按角色卡 / danbooru tags 生成人物。要求：
   - 构图与动作**按最终需求**来（反正是要贴进背景的那一个）；
   - 背景尽量干净（纯色 / 简单背景更好贴），分辨率的长宽比**与底图一致**（省一次拉伸）；
   - 只认返回的 `mediaIds` —— 没拿到产物就别进入阶段二。
2. **阶段二（局部重绘）**：
   - `comfy_use_media(mediaId=<阶段一产物>)` → 人物图的 `filename`；
   - 遮罩框在**人物该出现的位置**（拿不准先问用户，或先用大一点的框 + 低 denoise）；
   - 提交时 `20.image` = 人物图、`17.denoise` = **0.4~0.6**（高了人物会变样，低了接缝明显）。
3. **保住画风的四条**（用户的原话是"保持画风"）：
   - 阶段一与阶段二**同一个底模**（都 Anima 或都 Krea2）与同一套 CLIP / VAE；
   - 阶段二也挂**同一条画风 LoRA**：覆盖 Anima 模板里的节点 `4`（`lora_name` + `strength_model`
     + `strength_clip`，默认强度 0 = 不生效）；**覆盖前先确认本机有这个文件**
     （`comfy_env_report` / `list_dir`，缺了别编文件名）;
   - 阶段二的提示词**不要描述人物**（人物像素已经贴进来了）——只写遮罩区里该补什么、
     以及"光线 / 色调与周围一致"这类要求；
   - `denoise` 不超过 0.6，`feather` 8~24。

## 换背景保人物

同一个工作流 + `invert=true`（把"人物那块"翻过来 = 只重绘人物以外的一切），
提示词里写**新背景**（Anima 用 tag：`classroom, soft daylight`；Krea2 用自然语言）。
人物像素被 `SetLatentNoiseMask` 保住，所以不会被重画。

- 人物**边缘一圈**仍可能被重绘：`feather` 8~24，或者把 `region` 往外放大 3~5%（x/y 减一点、w/h 加一点）；
- 要"抠得干干净净"（带 alpha 的透明人物）得有分割 / 抠图模型（rembg 之类）：
  **本机有没有用 `comfy_env_report` 查**，没有就如实说做不到，别吹。

## 纪律

- **遮罩与底图尺寸必须一致**：`11.width/height`、`21.width/height` 一起覆盖成底图尺寸，
  不然采样直接炸（表现是"图糊 / 报尺寸错"，而用户看不懂）。
- 工作流文件**属于项目**（`<项目根>\comfyui\workflows\`），路径固定；改了节点再跑之前
  **重新 `comfy_load_workflow`** 拿新 promptId（旧 promptId 是旧图）。
- 拿不准就先用 `comfy_check_inputs(graph=…)` 看每个输入齐了没；
  提交前也可以用 `comfy_audit_workflow` 审一遍（参数档位 / 加载器目录那几项）。
- **产物只认 `mediaIds`**：没拿到就说没拿到，**不许**说"我换好了"。
- 抽卡：**保留 `filename_prefix`、只换 `seed`**（前缀不变才攒得住同一批）。
  ⚠ **Krea2 那份对 seed 明显更敏感**（2026-09-26 实测：同一份底图 / 同一张遮罩 / 同一段提示词、
  只换 seed 跑 3 次 —— **1 次画出了要的人，2 次把重绘区里的灯柱、围墙照旧渲染了一遍**；
  同一天 Anima 那份每次都跟随提示词）。所以写实档一次不准**先换 seed 重抽 2~4 张**，
  别急着改工作流或重写提示词；要对照就**固定 seed 只改一个变量**，别把两件事混在一次里改。
  抽出来的每一张都**如实说清是哪一版**（`seed` 号 + `mediaIds`），让用户自己挑。
- Anima 那份的负面提示词在 cfg=1 时**几乎不起作用**（Turbo 档的物理事实）：
  要排除某个元素就写进**正面**的反面描述，或者提高 cfg 并接受质量变化。
- 用户在手机 / 平板上没法手画遮罩 —— 别让他去 ComfyUI 的 MaskEditor 里描，
  用 `comfy_make_mask` 的框 + 羽化表达意图，或者问他要一组坐标。
