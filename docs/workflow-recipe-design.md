# 工作流配方（recipe）：让 AI 学会"新模型该怎么接上去"

> **起因（用户 2026-09-26 的原话）**：
> 「我想可能很多模型都有 image edit 工作流，比如我刚在
> `D:\Comfy-Desktop\...\user\default\workflows` 里导入了 krea2 的图像编辑工作流。
> 甚至视频模型都有作为 image edit 基底模型的资质。如果以后出了新的模型，
> 如何让 AI 自主学习新模型泛用/特定工作流的搭建？」
>
> **一句话回答**：把"工作流"拆成**任务骨架（泛用）**+ **模型槽位绑定（特定）**+ **参数档位（特定）**
> 三层，让 AI 从**已经躺在磁盘上的具体工作流**里把前两层**归纳**出来、**落库沉淀**，
> 新模型出现时只需做"把骨架绑到新槽位"这一步 —— 而不是每次从零猜一张图。
>
> **本文是设计（为什么这么做、判据是什么、分几步落地）**；实施登记在
> [`implementation-master-plan.md`](implementation-master-plan.md) §12。

---

## 1. 现状盘点：这条路已经铺了 80%，缺的是"归纳 + 沉淀"

本机今天的工具已经能回答"某个具体工作流的**事实**"，但回答不了"这一族工作流的**形状**"：

| 已有能力 | 工具 | 它回答的问题 | 它答不了的 |
| --- | --- | --- | --- |
| 看得见 | `comfy_list_workflows` / `comfy_find_workflow` | 机器上/库里有哪几份工作流 | 它们是干什么的 |
| 读得进 | `comfy_load_workflow`（界面格式 → API 图，含 subgraph 展开 / 旁路 / 前端模拟） | 这份文件能不能变成可提交的图 | 这张图的**结构含义** |
| 审得出 | `comfy_audit_workflow`（七项固定检查） | 这份图有没有已知的坑 | 它的任务类型、模型角色 |
| 对得上 | `comfy_fit_workflow`（`ComfyModels.requirements` + `inventory`） | 图里点名的模型本机有没有、差哪几个 overrides 键 | 换一个**新模型**时该动哪几个槽位 |
| 查得清 | `comfy_check_inputs`（`WorkflowInputs.scan`） | 还等着外部给什么 | 哪些输入是"这个任务的可调旋钮" |
| 跑得动 | `comfy_submit`（R10：审批 → 提交 → 等 → `captureRun()` 入库） | 这一次跑出什么 | 上一次的**经验**（哪套骨架 + 哪个模型跑通过） |

**缺的那一环**：没有任何地方记住"`Krea2EditModelPatch` + `UNETLoader` + `CLIPLoader(type=krea2)`
+ `VAELoader` + `LoraLoaderModelOnly` 这条形状 = **参考图编辑**任务，它的可换件是底模/文本编码器/VAE/LoRA，
它的旋钮是 `TextInput_.text` / `LoadImage.image` / `KSampler.{steps,cfg,seed}`"。
于是**每一次对话都要重新推一遍**，而且推出来的结论**随对话结束蒸发**。

用户举的例子正是这条缺口的典型：krea2 的图像编辑工作流已经导进磁盘
（`krea2ImageEditingAnd_v10.json` / `Krea+2+Identity+Edit+V1.2单·双图像编辑.json`），
但"把它当基底去接下一个新模型"今天只能靠模型即兴发挥。

---

## 2. 三层分解（本设计的地基）

任何一份具体工作流，都能被拆成三层。**判据全部取自代码里已有的事实表，不新造一套猜测规则。**

### 2.1 骨架（skeleton）—— 泛用，与模型无关

- **定义**：执行相关节点的**类集合** + **类级连线形状**（`上游类.输出 → 下游类.输入`）。
  丢掉节点 id、丢掉纯界面节点（`Note` / `Reroute` / `Label (rgthree)` / `Bookmark` / `Anything Everywhere`…）。
- **指纹**：`skeleton_hash = sha256(排序后的类集合 ‖ 排序后的类级边)[:16]`。
  两份工作流只要形状一样，指纹就一样 —— **这就是"泛用"的可计算定义**，
  也是"同一任务的第 2、3 份工作流"能被归到一族里的依据。
- **不变的判据**：类级边按 `(下游类.输入名, 上游类.输出槽位)` 记，**不记节点 id**（id 每份文件都不同）。

### 2.2 模型槽位（slots）—— 特定，可换件

- **定义**：图里"从磁盘加载一个模型文件"的那些输入。
- **判据**：直接复用 [`ComfyModels.requirements`](../server/src/main/kotlin/com/comfyhub/ComfyModels.kt)
  —— 它已经有那张"哪个类的哪个输入是一个模型文件名"的**确定**表
  （`CheckpointLoaderSimple.ckpt_name` / `UNETLoader.unet_name` / `CLIPLoader.clip_name` /
  `VAELoader.vae_name` / `LoraLoader*.lora_name` / `ControlNetLoader.control_net_name` /
  `UpscaleModelLoader.model_name` …），并且能用 `/object_info` 现算出**第三方加载器**。
  值必须是**字符串**（是数组=连线，跳过）—— 这一条是既有纪律，继承。
- **角色**：用 `Requirement.folder`（`checkpoints` / `diffusion_models` / `text_encoders` /
  `vae` / `loras` / `controlnet` / `upscale_models` …）当角色名。**不新建角色表**。
- **覆盖键**：`Requirement.overrideKey`（`"<节点id>.<输入名>"`）就是 `comfy_submit(overrides=…)` 的键。

### 2.3 参数档位与旋钮（knobs）—— 特定，但通常不需要换

- **定义**：非连线（字面量）输入 —— 提示词 / 尺寸 / `steps` / `cfg` / `sampler_name` / `scheduler` /
  `seed` / `denoise` / 图片文件名 …
- **分类**（只为让模型看得懂，**不替它决定值**）：
  `prompt`（文本编码节点的 `text`）· `image`（`image` / `mask` 这类文件名）· `sampling`
  （`steps` / `cfg` / `sampler_name` / `scheduler` / `denoise`）· `seed` · `size`（`width` / `height` /
  `batch_size`）· `other`。
- **纪律**：档位建议（"turbo 通常 cfg≈1，此时负向几乎不起作用"）由 `comfy_audit_workflow` 与
  skill 说，**配方只记事实**（当前值是多少、键是什么）。

### 2.4 任务分类（task）—— 判据是"输出与输入位置"，不是节点名

按下面的**顺序**判（先命中先算），每条都对着真文件核过（见 §5 的实测清单）：

| 顺序 | 判据 | 任务 |
| --- | --- | --- |
| 1 | 有 `SaveAudio*` / 音频节点 | `audio` |
| 2 | 有 `SaveVideo` / `VHS_VideoCombine` / 视频采样节点（`MiniMaxH3*` / `Wan*` / `SVD*`） | `video` |
| 3 | 有"参考图编辑"链（`Krea2EditModelPatch` / `Krea2OstrisEditModelPatch` / `Krea2EditGroundedEncode` / `TextEncodeKrea2OstrisEdit` / `ReferenceLatent` / `FluxKontext*`）且图像输入 ≥1 | `edit` |
| 4 | 有遮罩链（`LoadImageMask` / `ImageToMask` / `SetLatentNoiseMask` / `InpaintModelConditioning` / `GrowMask`，或任何节点上真的连着一个 `mask` 输入） | `inpaint` |
| 5 | 只有一个放大模型、没有采样器 | `upscale` |
| 6 | 图像输入 ≥1 | `i2i` |
| 7 | 其余 | `t2i` |
| — | 一个都没命中（空图 / 认不出） | `unknown`（**如实说**，不硬塞一档） |

> ⚠ 顺序里有两处**不能换**（换了就静默判错，且看起来一切正常）：
> ① `audio` / `video` 在最前 —— 输出即事实；
> ② **`edit` 在 `inpaint` 前** —— Krea 2 的 Identity Edit 两条链都有
> （实测：那两份工作流里既有 `Krea2Edit*` 又有 `ImageToMask` / `GrowMask`），
> 把它判成 `inpaint` 的话 `comfy_list_recipes(task="edit")` 就永远找不到用户的图像编辑工作流。
>
> ⚠ 任务分类是**提示**不是结论：关键判据（有视频输出 / 有音频输出 / 有遮罩）是**事实**，
> 所以 `video` / `audio` / `inpaint` 三档可信度最高；`edit` 与 `i2i` 的边界
> （有参考图但没有遮罩）本身就是连续谱，认错只影响提示词，不影响可执行性。

---

## 3. 学习闭环：新模型出现时 AI 该走的六步

```
① 发现 ──▶ ② 归纳 ──▶ ③ 校验 ──▶ ④ 绑定 ──▶ ⑤ 跑 ──▶ ⑥ 沉淀
（看见文件） （学成配方） （本机缺什么）（接到新模型）（comfy_submit）（写回配方库）
```

| 步 | AI 的动作 | 用的工具（今天 / 本次新增） | 事实来源 |
| --- | --- | --- | --- |
| ① 发现 | 列本机工作流、按文件名/任务找 | `comfy_list_workflows`（已有）、`comfy_list_recipes`（**新**） | `ComfyWorkflowFiles` 三个目录来源（配置的输出目录父目录 → `ComfyLocator` → `ComfyRoots`） |
| ② 归纳 | 把一份具体工作流学成配方 | `comfy_learn_workflow`（**新**） | `WorkflowConvert` 转换后的 API 图 + `ComfyModels.requirements` |
| ③ 校验 | 节点类齐不齐（`/object_info`）、模型在不在（清单并集） | `comfy_fit_workflow` / `comfy_env_report` / `comfy_audit_workflow`（已有） | `ComfyModels.inventory` 的 `visibleToInstance`（"盘上有、实例看不到"要单列） |
| ④ 绑定 | 把配方接到新模型上，拿到**覆盖键** | `comfy_bind_recipe`（**新**） | 配方的槽位 + 本机清单 + `WorkflowFit` 的四档结论 |
| ⑤ 跑 | 提交（审批 → 等 → 入库） | `comfy_submit`（已有，**一个字不改**） | R10 全链路 |
| ⑥ 沉淀 | 把"这套骨架 + 这个模型"记下来 | `comfy_list_recipes`（**新**，按骨架分组/按模型查） | 配方库 + `capture_runs` |

**两条边界（不许越）**：

1. **配方不是工作流**。配方里没有节点 id 的可执行图、也不是提交对象 ——
   提交永远走 `comfy_submit(promptId=…, overrides=…)`（R10 一条不变：
   审批 → 提交 → 等 → `capture.captureRun()` 入库）。配方只是**给模型看的知识**。
2. **只报不改**（沿用 `comfy_fit_workflow` 的纪律）。`comfy_bind_recipe` 产出的是
   "建议写进 overrides 的键值 + 缺口清单"，**它自己不提交、不写用户的文件、不改工作流**。
   真正动手的是那次 `comfy_submit`（还要过审批）。

### 3.1 新模型出现的三种情形（这才是用户问的那件事）

| 情形 | 例子 | AI 该做什么 | 本次能做到吗 |
| --- | --- | --- | --- |
| **A. 同骨架、换底模** | 出了 `krea2-turbo-v11`，还是 unet+clip+vae+edit-patch 那套 | `comfy_bind_recipe(recipe, model=…)` → 拿到 `{"73.unet_name":…, "91.vae_name":…}` → `comfy_submit` | ✅ 本次就做 |
| **B. 同任务、不同加载器形态** | 新模型是**整包 checkpoint**，而手里的编辑骨架是 unet+clip+vae 三件套 | 先看 `comfy_list_recipes(task="edit")` 里有没有**用 checkpoint 的**同族骨架；没有就如实说"现有骨架接不上"，给用户两条路（找分离式版本 / 等骨架改写） | ⚠ 本次只**如实报**，骨架改写（节点增删 + 重连线）留下一项 |
| **C. 新任务范式** | 出了个"视频模型当图像编辑基底"的玩法（参考图 + 视频采样器） | 先在磁盘上找有没有人这么用的工作流（①）；没有就 `civitai_search` 找它官方/社区工作流 → 让用户用 ComfyUI 打开并保存 → ① 重来一遍 | ✅ 流程成立（学习入口就是文件），**但没有的骨架就是没有**，不许编 |

> 情形 C 就是"视频模型能不能当图像编辑基底"的答案：**能不能，由磁盘上/网上有没有那条骨架决定**；
> AI 的职责是**把它找出来并学下来**，不是凭"视频模型理论上也能吃参考图"去拼一张没人跑通过的图。

### 3.2 泛用 vs 特定的落点（一张表）

| | 泛用（跨模型复用） | 特定（换模型要动） |
| --- | --- | --- |
| **骨架** | 任务链的类与连线、遮罩语义、输出节点 | — |
| **槽位** | 槽位的**角色**（底模 / 文本编码器 / VAE / LoRA / 控制网 / 放大） | 槽位里的**文件名** + 覆盖键里的**节点 id** |
| **档位** | 采样链的参数名（steps/cfg/sampler/scheduler/denoise） | 具体数值、`CLIPLoader.type`（krea2 / qwen_image …）、LoRA 权重 |

---

## 4. 分步落地

| 步 | 内容 | 验收判据 | 依赖 |
| --- | --- | --- | --- |
| **W1** ✅ | `WorkflowRecipe` 纯解析核心：任务分类 + 骨架指纹 + 槽位（含 `roleGuessed`）+ 旋钮 + 输出节点 + `render()` / `toJson()` —— **已落地（`WorkflowRecipe.kt` + 11 例单测）** | 单测覆盖四种任务的**真形状夹具**（t2i / i2i+遮罩 / edit / video）；骨架指纹对"同形不同 id / 不同文件名 / 界面节点"稳定、对"多一个结构节点 / 多一个 LoRA"敏感；仓库里两份真 inpaint 工作流能归纳出槽位与遮罩（`gradle test` 655 例全绿） | 无 |
| **W2** ✅ | `workflow_recipes` 表 + 仓储 + `GET/POST/DELETE /api/ai/workflow-recipes` —— **已落地（2026-09-26）**：幂等靠 `source_ref`（`file:<sha>` / `prompt:<id>` / `graph:<sha>`，与 `comfy_load_workflow` 同一把钥匙）；`prompt_id` 是 `ON DELETE SET NULL`；**不进同步白名单**；`complete` 与 `gaps` 如实记"骨架缺件" | 单测 9 例（幂等 / 整行更新 / 删提示词后配方还在 / 不进账本 / complete 过滤 / 详情还原出可渲染的配方 / 报错不截断 / 真文件学一遍再读回）；**真库副本 + 真 ComfyUI 抽查**（见总纲 §12） | W1 |
| **W3** ✅ | 三条工具 `comfy_learn_workflow` / `comfy_list_recipes` / `comfy_bind_recipe` + `WorkflowRecipeBinder`（**只报不改**，四档结论）+ 接线 —— **已落地（2026-09-26）** | `ToolRegistryTest` 5 例（参数与路径准入下传 / 稳定码翻成工具失败码 / 空库指到学习入口 / 缺参数 / extra 翻成请求）+ `WorkflowRecipeBinderTest` 11 例（见总纲 §12）；`gradle test` 680 例全绿 | W2 |
| **W4** | 系统提示 v31 第 20 条 + skill `workflow-recipe` + e2e 脚本 + 文档登记 | `SystemPromptNsfwTest` 盯第 20 条；e2e 对着**真工作流文件**跑一遍"学 → 列 → 绑" | W3 |
| **W5**（下一项，未做） | **骨架改写**：unet+clip+vae ↔ 整包 checkpoint 的节点增删与重连线（要 `/object_info` 的输出槽位 + 重连线后过 `comfy_submit` 的预检） | 真 ComfyUI 上跑通一次"整包 checkpoint 接进编辑骨架" | W3 |

---

## 5. 实测依据（写这份设计时对着真文件量的，不是推测）

只读扫了用户 `…\user\default\workflows\` 下的 8 份文件（脚本是临时的，在 `.run\` 下、不进版本库）：

| 文件 | 形状（节点数 / 类数） | 这次设计从它拿到的判据 |
| --- | --- | --- |
| `krea2ImageEditingAnd_v10.json` | 18 / 16 | `edit` 档的判据（`Krea2OstrisEditModelPatch` + `TextEncodeKrea2OstrisEditx2` + `FluxKontextMultiReferenceLatentMethodx2`）；槽位 = `UNETLoader` + `VAELoader` + `CLIPLoader(type=krea2)` + `LoraLoaderModelOnly` |
| `Krea+2+Identity+Edit+V1.2单图像编辑.json` | 25 / 22 | 遮罩链（`ImageToMask` / `GrowMask` / `ImageCompositeMasked`）也要进 `inpaint` 判据 |
| `Krea+2+Identity+Edit+V1.2双图像编辑.json` | 19 / 15 | 双 `LoadImage` = 图像输入计数**按 LoadImage 类数**（不是按 `image` 输入名） |
| `krea2SFWNSFWUncensoredImageTo_v10.json` | 113 / 41 | 骨架指纹必须**丢掉前端节点**：这一份里 `Bookmark`×8 / `Label`×11 / `GetNode`×20 / `SetNode`×16 / `Note`×7 / `Anything Everywhere`×2；subgraph 是 UUID 类名（必须先经 `WorkflowConvert.flatten` 才会消失） |
| `anima11I2IFaceHand_v10.json` | 52 / 25 | 人脸修复链（`FaceDetailer`×4 + `SAMLoader`×2 + `UltralyticsDetectorProvider`×6）说明**槽位不止底模**：`SAMLoader` / detector 也是模型文件 |
| `video_minimax_h3_i2v.json` | 10 / 7 | `video` 档判据（`SaveVideo`）；同时证明"视频工作流里那个 `LoadImage` 就是它的图像输入" |
| `minimaxMusic3ComfyuiSong_v10.json` | 4 / 3 | `audio` 档判据（`SaveAudioAdvanced`） |
| `image_anima_lllite_any_control_to_image.json` | 11 / 10 | 只有 11 个节点也是完整工作流：**节点数不能当"学得对不对"的判据** |

> 用户点名的 krea2 图像编辑那两份**都在**这个目录里 —— 所以"学习入口是文件"这条路，
> 在用户的机器上现在就能立刻跑通（不需要他再导出一次 API 格式）。

### 5.1 W1 落地时那次真机抽查（16 份文件全扫了一遍，抓出三件事）

`WorkflowRecipe.analyze` 写完之后，用一个**临时探针**（跑完就删）把上面那个目录里
**全部 16 份**文件都过了一遍：UI 格式的经 `WorkflowConvert` 转成 API 图，
再用 `/object_info`（本机实例当时有 2817 个节点类）现算模型槽位。
16 份全部归纳成功（任务分别为 t2i / i2i / inpaint / edit / video / audio），三件事是**只有真文件才摆得出来**的：

| 发现 | 现象 | 落点 |
| --- | --- | --- |
| **① 统计表把"次数"写成了类名** | 我第一版的 `EDIT_NODES` 里写着 `TextEncodeKrea2OstrisEditx2` —— 那不是类名，是我自己那张统计表里"出现 2 次"的**后缀**。于是 Krea 2 的两份图像编辑工作流被判成 `inpaint` / `i2i` | 类名**逐字对着真文件的 `class_type` 抄**（`Krea2EditGroundedEncode` / `TextEncodeKrea2OstrisEdit`）。⚠ 教训：**别拿"汇总过的统计"当原始事实**（汇总已经把 key 和值混在一起了） |
| **② 自定义加载器的"目录名"是猜的** | `UltralyticsDetectorProvider.model_name` 被标成 `diffusion_models`，而它的真身是 `models/ultralytics/bbox` 下的检测器权重 —— 一个会把人指到错地方的结论（8 个槽位全这样） | `Slot.roleGuessed`：拿"只走内置确定表"的那一遍与"含 `/object_info` 现算"的那一遍做**差集**，差出来的就是猜的；`render()` 里用一句话如实说明。**不许**硬塞目录名 |
| **③「学到一条缺了节点的骨架」是静默的** | 那几个 Krea2 编辑节点（`Krea2EditModelPatch` 等）**不在当前跑着的这个实例里**（2817 个类里一个都没有）→ `WorkflowConvert` 把它们当"前端节点"**摘掉**，转换结果是 19 个节点、编辑链整个没了。如果就这么学下去，配方会是一条**看着像、其实缺了编辑链**的骨架，而**没有任何报错** | 学习入口必须**先看转换报告**：有 `unsupportedNodes` / `unresolvedInputs` 时**默认拒学**，并把"本机这个实例没有这些节点类"说清楚（正确动作是让用户装/启用那几个自定义节点，或换实例）；显式 `tolerateUnsupported=true` 才学，且配方上标"骨架缺 N 个节点类"。另：转换时"控件值个数与输入声明对不上"的警告（真机上 10 个节点命中）也要带进配方的 `warnings` —— 那种节点的参数是**错位**的（实测 `42.steps = 105496379526884`） |

---

## 6. 明确不做（诚实清单）

| 不做 | 原因 |
| --- | --- |
| **无人值守的"自学习"**（后台定期扫盘、自动学所有工作流） | 学习必须由一次真实需求驱动（用户要干什么 / 新模型要接哪条骨架）。自动学一屏工作流只会把配方库灌满噪声，而"哪条骨架适合这个模型"仍要人判断 |
| **代下模型** | 用户拍板（`find_model` 那条纪律）：只给链接 + sha256 + 目标目录 |
| **把配方当成可提交对象** | R10：提交永远走 `comfy_submit`（审批 + 等 + 入库），配方只提供"覆盖键" |
| **猜第三方节点的语义** | 槽位只认 `ComfyModels` 那张确定表 + `/object_info` 现算的枚举；认不出的类**如实列进 `unknownSlots`**，不猜 |
| **跨模型"自动等价"**（把 A 底模的 LoRA 自动换成 B 底模的） | 字面相似≠架构兼容（`WorkflowFit` 已有的结论）；换 LoRA 一律走"先问用户" |
| **骨架改写的"智能"版本**（让模型自由增删节点） | W5 只做**机械改写**（三件套 ↔ 整包），且必须过 `/object_info` 必填输入校验 + `comfy_submit` 预检；自由增删等于让它手搓一张没人跑通过的图 |

---

## 7. 与既有纪律的关系（改这块之前先读）

- **工具清单只有一个真源**：三个新工具加在 `ToolRegistry` 的 `builtinTools` 里，
  `GET /api/ai/tools` / 系统提示 / 界面全部从它派生（AGENTS §10、红线 R12）。
- **系统提示改动必须 bump `SystemPrompt.VERSION`**（v31），并在 `SystemPromptNsfwTest` 里留一条断言。
- **配方不进同步白名单**（`sync.rs` 的 `SYNCED` 与手机侧镜像表**都不要动**）：
  它是桌面侧的知识，手机提交要走 `comfy_queue_submit`（R11），拿到一份它跑不了的配方只会误导模型。
  这一点与 `character_cards` 同款，理由要写在建表注释与总纲 §3.3 的对照里。
- **写库走仓储**（`Db.tx` + `BEGIN IMMEDIATE`，红线 R2）、**不用 `INSERT OR REPLACE`**（R3）、
  **`updated_at` 靠触发器**（P2 起的约定）。
- **路径一律走 `ToolPolicy.resolveRead`**：学习入口接受文件路径，与 `comfy_load_workflow` 同一条准入。
- **不许假装**：拿不到 `/object_info` 就不判"本机没有这个节点类"；
  认不出任务就回 `unknown`；骨架改写接不上就如实说接不上。
