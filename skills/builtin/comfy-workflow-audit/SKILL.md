---
name: comfy-workflow-audit
description: 审核 ComfyUI 工作流：把"跑不起来"与"大概率白跑"的问题在提交前查出来，并给出修法。含七项固定检查（纯前端节点 / 节点版本不兼容 / 输出前缀+固定 seed 撞名 / 参数档位不匹配模型 / 显存风险 / 提示词风格与模型不符 / 加载器与目录不匹配）、对应的 `comfy_audit_workflow` 判据、以及每项的修法与话术。用于用户给了一份没跑过的工作流文件、要改一份工作流、或抱怨"跑完没出图 / 白跑了"时。
whenToUse: 用户给了一份本地工作流文件（或从网上抄来的工作流）要跑、要改之前；用户说"跑完没出图""白跑了""mediaCount 是 0""0.01 秒就结束了"；你要审一份 API 节点图；`comfy_submit` 被预检拒了想知道为什么。
version: 1
---

# 审核一份 ComfyUI 工作流

> **别为了"修好"另写一份。** 用户 2026-10-21 的纪律：从现有工作流出发；缺节点包就让用户启停 /
> 换实例，缺模型就让用户下，接线错了用 `comfy_prune_workflow` / `comfy_submit(connections=…)`
> 在**这次提交**上修；要永久改就存副本。原文件一个字节都不许改。

**目标**：在**烧掉显卡时间之前**发现"跑不起来"和"跑完拿不到东西"的问题。

**分工**（别越界）：

| 问题 | 用哪个工具 |
| --- | --- |
| 图的结构有没有毛病（本 skill） | `comfy_audit_workflow(graph=…)` |
| 图里要的模型用户机器上有没有 | `comfy_fit_workflow(graph=…)` |
| 输入齐不齐（缺图 / 空连线） | `comfy_check_inputs(graph=…)` |
| 三个模型根对不对（盘上有但实例看不到） | `comfy_env_report` |
| 摘节点 / 打补丁 | `comfy_prune_workflow` |

**顺序**：`comfy_load_workflow(includeGraph=true)` → `comfy_fit_workflow` → `comfy_audit_workflow`
→ （有问题就修）→ `comfy_submit`。

**提交时 `comfy_submit` 会自动跑一次审核，但只拦"必跑不了"的三项**（纯前端节点、版本不兼容节点、
用户机器上没有的节点类）与模型那一项。参数档位、显存、提示词风格**只提醒不拦** ——
那些是"不推荐"，不是"跑不了"，别拿它们去拦用户。

---

## ① 纯前端节点（BLOCK）

**长什么样**：`10 Label (rgthree)`、`163 Bookmark (rgthree)`、`15 Fast Groups Bypasser (rgthree)`、
`SetNode` / `GetNode`、`Anything Everywhere` 一族、`PreviewImage` / `ShowText` 这类显示节点。

**为什么跑不了**：这些类**只存在于 ComfyUI 前端**，服务端根本没有。整片节点会在验证阶段被丢掉，
表现是 `Prompt executed in 0.01 seconds`、零产物 —— 用户看到的是"AI 说跑完了，画廊里什么都没有"。

**修法**：`comfy_load_workflow` 读**原文件**（它会照前端排队前的行为等价改写：
广播填空、丢显示节点、补 SaveImage）；已经读进库的用 `comfy_prune_workflow` 摘掉，
照它给的 `openInputs` 用 `comfy_submit(connections=…)` 补线。
常见案例：一份带了大量 rgthree 前端节点的工作流，能带 20 个以上这类节点。

## ② 节点版本不兼容（BLOCK）

**长什么样**：`WidgetToString`、`SystemNotification`（`|pysssss` 变体）。

**判定**：装了、`/object_info` 里也有，但一跑就崩 —— 实测报
`IS_CHANGED() missing 1 required positional argument`。同样是 0.01 秒空跑。

**修法**：先在 ComfyUI-Manager 里**更新**这几个节点包再试；更新后还报就换等效节点
（例如用 `easy showAnything` / `PreviewAny` 代替 `WidgetToString`）或摘掉那条链。
**别对用户说"这个工作流坏了"** —— 说"这个节点包版本旧了，更新一下就能用"。

## ③ 输出前缀 + 固定 seed（WARN，但后果最容易被误读）

**长什么样**：`SaveImage.filename_prefix = "Krea2_spine"` 且 `KSampler.seed = 20260921`。

**为什么等于白跑**：文件名不变 → 覆盖同名文件 → 本项目按 `prompt_id` + 文件 SHA-256 判重 →
`mediaCount: 0`、提示"与已有文件相同，未重复入库"。**图其实在磁盘上，但用户看不到**。
现场：那次白跑 78 秒。

**修法**：分两种意图，别搞混 ——

- **抽卡**（用户说"再出几张""这张不行""多来几张挑一张"，或者第一张不满意让你重跑）：
  **保留 `filename_prefix`、只换 `seed`**。前缀不变才攒得住"同一批抽卡"，
  用户才能在画廊里一眼看出这一批是他刚才那轮。用户说"随机"时就把 seed 换成随机值
  （不写死数字），否则给一个新的具体数字（写死一个新数字更好复现）。
  **不要**为了避开重名去改前缀 —— 那会把同一批抽卡打散成一堆认不出来的碎片。
- **只跑一次定稿**（用户只要一张）：换 seed 或换 prefix 都行，随你。

**汇报时要如实说**："图在磁盘上但没入库"，不要含糊成"生成失败"；
抽卡那几轮要说清"这一批的 seed 分别是多少"，用户才能挑中想要的那张。

## ④ 参数档位 ≠ 该模型的官方档位（WARN）

| 模型（按文件名认） | 档位 | 备注 |
| --- | --- | --- |
| Krea 2 Turbo（`krea2…`） | 8 步 / cfg 1 / euler·er_sde | mu 1.15；挂了 4-step 蒸馏 LoRA 才用 4 步 |
| Anima（`anima…`） | er_sde·euler / cfg 1（合并底模常见 30 步 / cfg 4） | tag 模型 |

**修法**：按官方档位改，或者确认"就是要偏离"。**不认识的模型不判** —— 宁可不说，
也不要给一个会让人改错方向的数字。

## ⑤ 显存风险（WARN，只估不拦）

显存风险按**用户机器的实际显存**估（用 `comfy_get_status` / `comfy_env_report` 报的数，
别写死档位）。风险叠加项：出图尺寸 ≥4MP、带放大/重采样节点、
挂 3 个以上 LoRA。三者叠加时小显存（≤8GB）会 OOM 或退化到共享内存（慢十倍以上）。

**修法**：降一档（尺寸 / 放大倍数 / LoRA 数量三者择一）；
或者用 `comfy_prune_workflow` 往模型链上补 Sage Attention（用户偏好，省显存）。

## ⑥ 提示词风格与模型不匹配（WARN / INFO）

- **Krea 2**：官方推荐**自然语言**，`masterpiece, best quality` 这类咒语没有意义。
  看到"逗号一大堆、没有成句结构"就当心 —— 那多半是给 Anima 写的 tag 稿直接拿去用了。
- **Anima**：**Danbooru tag 为主**（外观 / 动作 / 表情写 tag 更稳），自然语言只补氛围。

**修法**：按 `krea-2` / `anima-prompt` skill 重写对应那一版；两份词不要混用。

## ⑦ 加载器与目录不匹配（WARN）

**长什么样**：`UNETLoader.unet_name = "krea2TurboOfficialComfy_krea2TurboInt8.safetensors"`
而这份文件躺在 `checkpoints/` 里（官方模板走 `UNETLoader` + `diffusion_models/`）。

**为什么**：ComfyUI 的枚举是**按目录**给的 —— `UNETLoader` 的下拉里根本没有 `checkpoints/`
里的文件，提交只会报"文件不存在"。

**修法**：换成目标目录里真实存在的文件（`comfy_fit_workflow` 会给候选），
或者换用读那个目录的加载器（`CheckpointLoaderSimple` ↔ `UNETLoader`）。
**别自己拼一个文件名** —— ComfyUI 只认磁盘上真实存在的文件。

---

## 汇报纪律

1. **结论先行**：哪几处"跑不起来"、哪几处"不建议"，各是什么，别把七项平铺给用户。
2. **只报不改**：要动图就明说改哪几个键（`overrides` / `removeFrom`），让用户能回退。
3. **拿不到的信息不许猜**：ComfyUI 没跑（拿不到 `/object_info`）时**不判**"用户机器上没有这个节点类"；
   不认识的模型不判参数档位。
4. **别把"不推荐"说成"跑不了"** —— 参数、显存、提示词风格那三项拦住用户提交是不对的。
