---
name: inpaint-composite
description: 图像编辑的局部重绘工作流 —— 保背景换人物 / 换背景保人物 / 把先生成好的人物贴进某张图。**「把图里的人换成 X」默认就是三件套**（① 用现成的 `comfyhub_t2i_anima.json` 生成替换目标 → ② 用 AI 软遮罩把它抠下来、贴上原图背景），别一上来就想一次重绘完事。**贴人物图只有一条路**：`comfyhub_paste_person_matte_anima.json`（AI 软遮罩：`easy imageRemBg` + `refine_foreground=true` 抠出连续 alpha，ring 由 `GrowMask` + `MaskComposite` 从同一份 alpha 现算，人物像素与画风原样保留）——⚠ **用之前必须先查 ComfyUI 里有没有抠图模型**（`comfy_get_status` 的 `mattingModels.usable`），缺了就指引用户下载 `BEN2_Base.pth` / `RMBG-1.4.pth` 到 `<ComfyUI 根>\models\rembg\`；**没有模型就是做不到，如实说**（颜色抠图那条退路已整档删除，别再想它）。纯重绘 / 换背景保人物用 `comfyhub_inpaint_anima.json`；底图与人物图用 `comfy_use_media` / `comfy_use_attachment` 投进 ComfyUI 的 input。krea2 那两份实测不适合做编辑（重绘不跟随提示词、贴人守不住），别用。遮罩一律"白=重绘区"。
whenToUse: 用户说"保持背景不变，换个人 / 换个动作 / 换身衣服"、"背景换掉但人物不变"、"把这张图里的人换成…"、"先生成人物再放进去"、局部重绘 / inpaint / 遮罩 / 贴人 / 合成人物 / 抠图 / 抠人 / 抠出来时。
version: 5
---

# 图像编辑：局部重绘（保背景换人物 / 换背景保人物）

## 版本记录

- **version 5（2026-09-28）**：把"**换人 = 三件套**"提到最前面，写成一张能照着走的步骤表
  （① 现成 `comfyhub_t2i_anima.json` 生成替换目标 → ② AI 抠图 + 贴回原图背景，
  走 `comfyhub_paste_person_matte_anima.json`），并**明写"阶段一不要自己另写工作流"**。
  起因是用户报"模型不知道先生成一张替换目标、再 AI 抠图、最后贴上目标背景"，
  真机记录里它当场新写了一份 t2i 图，把轮数烧在侦察上，最后卡在上限、一张图都没出来。
- **version 4（2026-09-27）**：按用户拍板「**放弃纯rust编辑，删除它**」，**整档删掉"颜色抠图当合成遮罩"**——
  连带删掉的参数有 `matteSource`（显式传它会被**明确拒绝**并说明"已删除"）、`ring`、`shrink`、`edgeBand`、
  `edgeTolerance`、`light` / `lightStrength` / `shadowStrength` / `shadowOffset`，以及它们背后那套
  `drop_blended_edge`（按颜色收边）/ `erode_mask`（choke）/ `unmix_edge`（反混）/ `despill_edges`（去溢色）/
  Rust 现算的 `ring_mask` / 打光。理由：实测那样贴出来的人物**轮廓外面一圈青绿、发梢被整段吃掉**，
  而根因是 **Rust 手里没有模型的 alpha**，只能按颜色猜覆盖率 —— 调参救不了。
  现在**贴人物图只有 matte 工作流一条路**（抠图与前景估计全交给工作流），
  `comfyhub_paste_person_anima.json`（颜色双遮罩那份）随这一档一起删除。
  `comfy_make_mask` 的 `mode="subject"` 只回 `personFilename` + 定位器诊断，**不再写合成遮罩**；
  几何遮罩档（`mode="rect"` + `shape` / `region`）与摆放（`scale` / `x` / `y`）照旧。
- version 3（2026-09-27）：贴人物图首选 AI 软遮罩，并写清"先查模型、缺了指引用户下载"。

## ⚠ 结论先看：图像编辑**只用 anima 那份**（`comfyhub_inpaint_anima.json`）

**krea2 那份（`comfyhub_inpaint_krea2.json`）实测不适合做图像编辑** —— 2026-09-26 在活实例上量到的：

| 图像编辑要做的事 | anima | krea2 |
| --- | --- | --- |
| 按提示词重绘遮罩区（纯重绘） | 3 个 seed **每次都跟随**提示词 | 3 个 seed **只有 1 次**跟随；另两次把重绘区里原有的灯柱 / 围墙照旧渲染了一遍 |
| 贴人物图（`20.image` = 阶段一产物） | ✅ 走 **matte 那份工作流**（AI 软遮罩，2026-09-27 已在真实例上端到端跑通） | denoise 0.5 → 人物图那块**带一圈白框**（cfg 1.0 与 4.0 都一样）；denoise 0.85 → 白框没了但**换成了另一个人**（贴进来的人守不住） |

krea2 的**文生图没问题**（阶段一那张白底人物图就是它生成的，很好）；它是**不适合编辑**
（局部重绘 / 贴人）这一档。所以：

- 用户要"局部重绘 / 换人 / 换背景 / 贴人" → **anima 那份**（提示词写 Danbooru tag）；
- 用户要"摄影 / 写实风格" → **如实说明**：写实那条路目前只有 krea2 这份图，
  而它在编辑档上不可靠（上表），要做就得**抽卡 + 人工挑**，或者换一个写实的 inpaint 底模
  （先 `comfy_env_report` / `find_model` 查本机有没有）——**不要**硬用它然后说"做好了"；
- krea2 那份继续留着给**文生图**用（还有 `comfyui\krea2_sage_turbo_t2i.json`）。

**一句话**：局部重绘就是"**一张底图 + 一张遮罩 + 一段只描述遮罩区的提示词**"。
遮罩里的白 = 要重新生成的地方，黑 = 原样保留。两种需求**用同一份图，只是遮罩反过来**：

| 需求 | `region`（人物那块） | `invert` | `20.image`（人物图） | `17.denoise` |
| --- | --- | --- | --- | --- |
| **保背景换人物**（换人 / 换动作 / 换装） | 人物所在位置 | `false` | 底图自己 或 阶段一的人物图 | `1.0`（纯重绘）/ `0.3~0.45`（贴人物图，只融接缝） |
| **换背景保人物** | 人物所在位置 | **`true`** | 底图自己 | `0.85~1.0` |
| 整张重画（等价图生图） | `full` | `false` | 底图自己 | `0.4~0.7` |
| **两阶段**：先生成人物再换上去 | 不需要遮罩（用 matte 工作流） | — | **`comfy_make_mask` 回的 `personFilename`** | `0~0.45` |

## ★★ 换人 = **三件套**，不是"一次重绘"（先看这一节）

用户说"把图里的主要人物换成 X"（换角色 / 换装 / 换动作）时，**默认就是这三步**，
一步都不能省、顺序不能换。**不要**一开始就去画一张新图直接贴 —— 那样边缘一定会脏。

| # | 这一步做什么 | 用哪个工作流 | 得到的产物 |
| --- | --- | --- | --- |
| ① 生成替换目标 | 按角色卡 / danbooru tag 出一张**新的目标人物**（构图动作按最终需求） | **`comfyui\workflows\comfyhub_t2i_anima.json`** | 一张人物图（`mediaIds`） |
| ② AI 抠图 + 贴回目标背景 | 用 AI 软遮罩把①里的人抠出连续 alpha，合进**原图** | **`comfyui\workflows\comfyhub_paste_person_matte_anima.json`** | 换好人的成图 |

> ⚠ **阶段一不要自己另写一份工作流**（2026-09-28 真机教训）：模型当场新写了一份
> t2i 图，光是"查本机 lora / 查节点契约 / 找模型"就烧掉大半轮数，最后卡在轮数上限上，
> 产物一个都没出来。现成的那份已经把加载器（底模 / CLIP / VAE / Sage / turbo LoRA）
> 配齐了，**直接 `comfy_load_workflow` 它、覆盖几个键就行**。

**全流程大概只要 8~12 步**（照着走，别展开侦察）：

```
0. 查抠图模型在不在        comfy_get_status  → mattingModels.usable
                          （false 就指引用户下 BEN2_Base.pth / RMBG-1.4.pth，然后如实说做不到）
1. 原图进 input            comfy_use_attachment(attachmentId=<用户消息里的附件 id>)
                          → 拿 filename 与 width/height
2. 阶段一：加载现成 t2i    comfy_load_workflow(path="…\comfyhub_t2i_anima.json")
3. 阶段一：提交            comfy_submit(overrides={ "10.width": <原图宽>, "10.height": <原图高>,
                             "11.text": "<角色 tag 栈 + 动作 + 简单背景>", "12.seed": … }, wait=true)
                          ★ 长宽比**必须与原图一致**（省一次拉伸）；只认返回的 mediaIds
4. 人物图进 input          comfy_use_media(mediaId=<上一步的产物>)
5. 定位 + 摆放             comfy_make_mask(mode="subject", baseFilename=<原图 filename>,
                             subjectFilename=<人物图 filename>, scale/x/y…)
                          → 拿 personFilename（**不是**遮罩；这一档不写遮罩）
6. 阶段二：加载 matte 那份  comfy_load_workflow(path="…\comfyhub_paste_person_matte_anima.json")
7. 阶段二：提交            comfy_submit(overrides={ "10.image": <原图 filename>,
                             "20.image": <personFilename>, "17.denoise": 0.3,   // 纯合成填 0
                             "11.width"/"11.height"/"21.width"/"21.height": <原图尺寸>,
                             "4.lora_name"+"4.strength_model"+"4.strength_clip": <同一条画风 LoRA，确认文件在> })
```

**只有第 ② 步是"抠图"**：抠的是①里生成的**新人物**，贴回去的是**原图的背景** ——
用户的原话"先生成一张替换目标，再将其用 AI 抠图下来，最后贴上目标背景"就是这条链。
每一步的细节（提示词怎么写、denoise 取值、边缘怎么收）见下面各节。

## ★ 先想这一步：**有两条路比抠图更稳**（能用就别抠）

用户说"把生成的人物放进某张背景"时，**先考虑这两条** —— 它们**根本没有边缘问题**，
而抠图（不管多好的模型）永远要在边缘上做取舍：

1. **阶段一就按最终背景生成**（最稳）：
   阶段一**别出"纯色背景的人物"**，而是**拿底图当 i2i 起点**（或同底模 + 提示词里把场景写全），
   阶段二只做 `denoise 0.35~0.5` 的融合重绘 —— 光、色、透视天然一致，画风也是同一个底模。
   ⚠ 代价：**人物姿态会被底图约束**（用户要换大动作就不适合这一条）。
2. **`denoise 0` 纯合成**（一个像素都不改，画风 100% 保留）：
   只靠 matte 工作流那份 alpha 把人物贴上去，重绘区为空 —— 连接缝都不重绘。
   前提是 alpha 要准，所以这一条**必须走 matte 工作流**（见下一节）。⚠ 代价：**光照不融合**
   （按背景估光那一档观感从来没合格过，已整档删除，**别宣称能做**），近景会显假。

**什么时候才真的需要抠图**：底图/场景已经钉死（比如用户给的实拍照片）、人物要换大动作、
或者要产出"透明人物"素材。那时才走下面这一节（而抠图这一步永远交给模型，不交给 Rust）。

## 抠图：**先查模型，缺了让用户下**（贴人物图必读）

贴人物图的成败几乎全在"轮廓抠得干不干净"上，而这件事**只有抠图模型能做**：
判据是**连续 alpha（软遮罩）**，不是"删/不删"的硬剪影。
所以贴人物图**唯一**的路是 `comfyhub_paste_person_matte_anima.json`
（内部 `easy imageRemBg` + `refine_foreground=true`）。

> ⛔ **没有抠图模型时没有退路**：拿 Rust 按颜色抠出来的剪影当合成遮罩那一档
> **已整档删除**（2026-09-27，用户拍板）—— 别再"先用颜色抠一下凑合"，也别承诺"边缘干净"。

### ① 先查本机有没有（**每次要用之前都查，别假设**）

**一条命令就够**：`comfy_get_status()` → 看返回里的 **`mattingModels`**：

```json
"mattingModels": {
  "node": "easy imageRemBg",
  "priority": ["BEN2_Base.pth", "RMBG-1.4.pth"],
  "usable": true,                     // ← 这一条为 false 就是"缺模型"
  "checked": [                        // 每个候选 ComfyUI 根**各有什么**（如实列，不合并）
    {"root": "...\\ComfyUI-Shared",              "dir": "...\\models\\rembg", "files": []},
    {"root": "...\\ComfyUI-Installs\\ComfyUI\\ComfyUI",
                                                 "dir": "...\\models\\rembg",
                                                 "files": ["BEN2_Base.pth", "RMBG-1.4.pth"]}
  ],
  "download": { ... },                // 缺了要下哪个、多大、放哪（照它转述给用户）
  "note": "..."                       // 为什么不能硬跑
}
```

- **`usable=true`** → 可以走下面那条路（`checked` 里哪个 `files` 非空，就用那个 `dir`）。
- **`usable=false`** → **别硬跑**，照 `download` 那两条**指引用户下载**（见 ②）。

⚠ **两个坑别踩**（`mattingModels.checked` 已经替你把第一层排掉了，但要知道为什么）：

1. `models\rembg\` 这个**目录名**是 easy-use 写死的（它读 `folder_paths.models_dir` + `rembg`），
   **不是** ComfyUI 原生的 `models\background_removal\`（那是另一套节点 `LoadBackgroundRemovalModel`）；
2. ComfyUI Desktop 把"程序"与"共享数据（input / output）"分在两处，而**模型在安装目录那一份**
   —— 本机实测：`ComfyUI-Installs\ComfyUI\ComfyUI\models\rembg\` 里有两个模型，
   而共享那份 `ComfyUI-Shared\models\rembg\` 是**空的**。所以**只查一处就会误报"缺"**
   （这也是为什么 `checked` 是**每个根各报一行**，不是合并成一句"有/没有"）。
   `comfy_use_media` 返回的 `inputDir` 指向**共享**那一份，**别拿它的兄弟目录当模型目录**。

### ② 缺了就**如实告诉用户下哪个**（别静默下载，也别硬跑）

`mattingModels.usable=false` 时，把这张表给用户，并说清"不下会怎样"
（`download` 那个键里就是同样的话，可以直接转述）：

| 模型 | 体积 | 下到哪 | 从哪下 |
| --- | --- | --- | --- |
| **BEN2**（推荐） | 1.1GB | `mattingModels.checked[].dir` 那个目录下（`…\models\rembg\`） | HuggingFace `PramaLLC/BEN2` 仓库里的 `BEN2_Base.pth` |
| RMBG-1.4（够用 / 最快） | 169MB | 同上 | HuggingFace `briaai/RMBG-1.4` 仓库里的 `model.pth`（**要存成 `RMBG-1.4.pth`**） |

⚠ **为什么必须"先查再跑"**：那个节点在文件缺失时会**自己静默下载**（走 `torch.hub.download_url_to_file`），
国内常常很慢或直接失败，表现就是"**点了按钮没反应**"（历史坑）。要么先按上表放好文件再跑，
要么盯住 ComfyUI 控制台的下载行。easy-use 下载失败后会自动换 `hf-mirror.com` 再试一次。
文件放好**不用重启**（节点每次现读文件）。放好之后再 `comfy_get_status()` 确认一次 `usable=true`，
**别只看用户说"下好了"**。

### ③ 模型确实没有时

**如实说做不到**：告诉他下哪个模型（②那张表）、放到哪、多大，并说明"没有模型就没法把人干净地抠出来"。
可以顺带给一句友好的替代方案 —— **上面"有两条路比抠图更稳"里的第一条**
（阶段一就按最终背景生成；另一条 `denoise 0` 纯合成也要 alpha，所以这里只能推荐第一条）。
**绝对不许**硬跑一遍然后说"换好了"。

## 三步（顺序不要调换）

1. **底图进 ComfyUI 的 input 目录**（`LoadImage` 只能读那里面的文件）：
   - 画廊里的产物 → `comfy_use_media(mediaId=…)`；
   - 用户随消息发来的图 → `comfy_use_attachment(attachmentId=…)`；
   - **两者返回里一定要拿 `filename` 与 `width`/`height`**（后面遮罩和 ImageScale 都照它填）。
2. **生成遮罩**（两条路，按需求选一条）：
   - **纯提示词重绘 / 换背景保人物** → 画几何遮罩：
     `comfy_make_mask(baseFilename="<上一步的 filename>", region=…, shape=…, invert=…, feather=…)`
     - `region` 支持 `full` / `left` / `right` / `top` / `bottom` / `center` / `person`
       （人物竖框）或归一化的 `"x,y,w,h"`（例 `"0.30,0.05,0.42,0.90"`）；
     - `shape` 是 `rect`（默认）/ `ellipse`；**换背景保人物**就是"人物那块 region + `invert=true`"；
     - 结果里的 `approximate=true` 表示"框是我估的"——**要如实转述**，并请用户给一组新坐标（或放大框）；
     - 边缘要自然就 `feather=8~24`；
     - 返回里有 `filename` —— **拿它覆盖工作流里的 `LoadImageMask`**。
   - **贴人物图（阶段一产物）** → 交给 matte 工作流，用这个工具**只做定位 + 摆放**：
     `comfy_make_mask(mode="subject", baseFilename="<底图 filename>", subjectFilename="<人物图 filename>",
     scale=…, x=…, y=…)`
     - 这一档**不写合成遮罩**（返回里 `maskGenerated=false`、**没有 `filename`**）：
       合成用的 alpha 由工作流里的抠图节点现算，**不要去覆盖任何 `LoadImageMask`**（那份工作流里没有）；
     - 拿回的 **`personFilename`** 就是 `20.image` 要填的那张人物图（被清理 / 摆放过的就是它）；
       另外回的 `subjectFilename` / `subjectRatio` / `background` / `chromaKey` / `chromaAdvice` /
       `bgColorIgnored` / `locatorNote` 全是**定位器诊断**（颜色剪影现在**只剩算框与摆放这件事**，
       不参与画面）—— `subjectRatio` 偏大偏小、`chromaAdvice` 说背景太浅，都**只影响摆放准不准**，
       跟贴出来干不干净无关（那取决于实例上的抠图模型）；不要因为 `subjectRatio` 不好看就换做法。
     - **摆放**用 `scale` / `x` / `y`（人物像素与定位掩码一起搬）；摆了之后
       **必须**用返回的人物图文件名覆盖 `20.image`（还用原图就是"人在这儿、遮罩在那儿"的错位）。
     - ⛔ **老参数会被明确拒绝**：`matteSource` / `ring` / `shrink` / `edgeBand` / `edgeTolerance` /
       `light` 一律报"已删除"。别传、也别照旧描述去猜。
3. **提交**（工作流是项目自带的，路径固定，**别自己编节点**）：
   ```
   comfy_load_workflow(path="<项目根>\comfyui\workflows\comfyhub_inpaint_anima.json", includeGraph=true)
     → 拿 promptId（**纯提示词重绘 / 换背景保人物用这份**）
   comfy_load_workflow(path="<项目根>\comfyui\workflows\comfyhub_paste_person_matte_anima.json", includeGraph=true)
     → 拿 promptId（**贴人物图只有这一份**：AI 软遮罩，抠 alpha 与 ring 都在工作流里算）
     （krea2 那份见开头的结论：不适合做编辑）

   // 纯重绘 / 换背景保人物（几何遮罩）
   comfy_submit(promptId=…, overrides={
     "10.image":  "<底图 filename>",
     "20.image":  "<底图 filename 或 人物图 filename>",
     "12.image":  "<上一步的遮罩 filename>",   // 几何遮罩（贴人物图那一档没有这一项）
     "11.width": W, "11.height": H,          // 与底图**必须一致**
     "21.width": W, "21.height": H,
     "15.text":  "<只写遮罩区里应该出现什么>",
     "17.seed":  <换一个新的数字>,
     "17.denoise": 1.0                       // 保背景换人物=1.0；换背景保人物=0.85~1.0
   })

   // ★ 贴人物图（matte 那份工作流：不覆盖任何 LoadImageMask）
   // 第 0 步：告诉工具"这一档只做定位/摆放"，拿回 `personFilename` 与尺寸（见上面第 2 步）。
   // ⚠ 抠图与**前景估计**都由工作流里的 `easy imageRemBg(refine_foreground=true)` 做：
   //   它用**模型自己的 alpha** 把边界那圈"人 + 背景"的混合像素还原成发色。
   //   （我们自己用 Rust 按颜色猜过两版：压通道 → 青边、两侧反混 → 白光晕，都比模型差，已撤掉。
   //    所以**别**再往 Rust 侧加"预处理"的主意 —— 那里没有模型的 alpha，猜不准。）
   comfy_make_mask(mode="subject",
                   baseFilename="<底图 filename>", subjectFilename="<人物图 filename>")
     → 拿 `personFilename` 与 width/height

   comfy_submit(promptId=…, overrides={
     "10.image":  "<底图 filename>",
     "20.image":  "<上一步的 personFilename>",   // 工具回的那一份（摆放过的就是搬过的那份）
     "11.width": W, "11.height": H,      // 与底图**必须一致**
     "21.width": W, "21.height": H,
     "15.text":  "<只写**人物外面那一圈**该出现什么 —— 写背景，绝不能写人物>",
     "17.seed":  <换一个新的数字>,
     "17.denoise": 0.4                   // 0 = 纯合成一个像素都不改；0.3~0.45 只融接缝（推荐）
   })
   ```
   - 那份工作流里 `28 GrowMask.expand` 就是"人物外面那一圈的宽度"（默认 **14**）：
     想融更细的接缝就覆盖 `"28.expand": 6~10`；想让人物一个像素都不被动就用 `"17.denoise": 0`。
   - `easy imageRemBg` 的 `rem_mode` 默认是 `BEN2`；想快就覆盖成 `"27.rem_mode": "RMBG-1.4"`
     （前提是那个文件在盘上，见"抠图"那一节）。
   - Anima 那份的提示词是 `15.text` + 采样器 `17.*`（**Danbooru tag**，写角色前先按系统提示第 18 条查三处）。
   - 要是用户坚持用 krea2 那份：提示词节点是 `13.text`、采样器是 `15.*`（**自然语言**，别堆 tag），
     并且**先说清上表那两档风险**（抽卡 / 贴人守不住）。
   - 提交前可以先用 `comfy_check_inputs(graph=…)` / `comfy_audit_workflow` 体检一遍。

## 两阶段：先生成人物，再换上去（用户要的正是这个）

1. **阶段一（t2i）**：按角色卡 / danbooru tags 生成人物。
   **用现成的 `comfyui\workflows\comfyhub_t2i_anima.json`（`comfy_load_workflow` 加载它），
   不要自己新写一份工作流**（2026-09-28 真机：自己写会把轮数烧在侦察上，最后卡在轮数上限、
   一张图都没出来；那份现成的工作流加载器已经配齐）。要改的只有几个键：
   `10.width` / `10.height`（画布）、`11.text`（提示词）、`12.seed`、
   需要画风 LoRA 时覆盖节点 `4` 的 `lora_name` / `strength_model` / `strength_clip`。要求：
   - 构图与动作**按最终需求**来（反正是要贴进背景的那一个）；
   - 分辨率的长宽比**与底图一致**（省一次拉伸）；
   - 背景**不必是绿幕**（抠图交给模型，任意背景都行）；只有当你**要摆放**
     （`scale` / `x` / `y`）时，纯色背景会让定位器估的框更准 —— 但那**只影响框与摆放**，
     与画面上的抠图无关，别把它当硬要求；
   - 只认返回的 `mediaIds` —— 没拿到产物就别进入阶段二。
2. **阶段二（贴人物图）**：
   - `comfy_use_media(mediaId=<阶段一产物>)` → 人物图的 `filename`；
   - **先按上面"抠图"一节查模型在不在**（`comfy_get_status` 的 `mattingModels.usable`），
     缺了就**指引用户下载**（别静默下载、也别硬跑）；
   - 然后 `comfy_make_mask(mode="subject", baseFilename=<底图>, subjectFilename=<人物图>, scale/x/y…)`
     拿 `personFilename`，照上面"三步"里那份 overrides 提交 matte 工作流；
   - `denoise`：纯合成（一个像素都不改）= **0**；只融接缝 = 0.3~0.45。
     **判据**：要"保住人物的样子/画风"就别超过 0.45。
   - ⚠ **别用矩形遮罩去贴人物图**：`ImageCompositeMasked` 只认遮罩形状、**不做抠图**，
     用矩形就会把那张图的**像素矩形整块**贴进去（阶段一的背景一起进来 = 一圈白框/绿框）。
     贴人物图这条路**只有 matte 工作流一条**（alpha 由模型给）。
3. **保住画风的四条**（用户的原话是"保持画风"）：
   - 阶段一与阶段二**用同一个底模**（**都 Anima** —— 图像编辑这一档 krea2 不可靠，
     见开头那张表）与同一套 CLIP / VAE；
   - 阶段二也挂**同一条画风 LoRA**：覆盖 Anima 模板里的节点 `4`（`lora_name` + `strength_model`
     + `strength_clip`，默认强度 0 = 不生效）；**覆盖前先确认本机有这个文件**
     （`comfy_env_report` / `list_dir`，缺了别编文件名）；
   - 阶段二的提示词**不要描述人物**（人物像素已经贴进来了）——只写人物外面那一圈该补什么、
     以及"光线 / 色调与周围一致"这类要求；
   - `denoise` 不超过 0.45。

## 换背景保人物

同一个工作流 + `invert=true`（把"人物那块"翻过来 = 只重绘人物以外的一切），
提示词里写**新背景**（Anima 用 tag：`classroom, soft daylight`；Krea2 用自然语言）。
人物像素被 `SetLatentNoiseMask` 保住，所以不会被重画。

- 人物**边缘一圈**仍可能被重绘：`feather` 8~24，或者把 `region` 往外放大 3~5%（x/y 减一点、w/h 加一点）；
- 要"抠得干干净净"（带 alpha 的透明人物）就上 **AI 软遮罩**（`easy imageRemBg`）：
  **先按上面"抠图"那一节查模型在不在**，缺了就**指引用户下载**（给 URL / 体积 / 目标路径）；
  模型确实没有时**如实说做不到**，别吹，也别拿别的东西硬充当"干净"。

## 纪律

- **遮罩与底图尺寸必须一致**：`11.width/height`、`21.width/height` 一起覆盖成底图尺寸，
  不然采样直接炸（表现是"图糊 / 报尺寸错"，而用户看不懂）。
- 工作流文件**属于项目**（`<项目根>\comfyui\workflows\`），路径固定；改了节点再跑之前
  **重新 `comfy_load_workflow`** 拿新 promptId（旧 promptId 是旧图）。
- 拿不准就先用 `comfy_check_inputs(graph=…)` 看每个输入齐了没；
  提交前也可以用 `comfy_audit_workflow` 审一遍（参数档位 / 加载器目录那几项）。
- **产物只认 `mediaIds`**：没拿到就说没拿到，**不许**说"我换好了"。
- 抽卡：**保留 `filename_prefix`、只换 `seed`**（前缀不变才攒得住同一批）。
  ⚠ **Anima 那份基本不用抽卡**（实测 3 个 seed 每次跟随提示词）；**krea2 那份才要抽**
  （3 个 seed 只有 1 次跟随）—— 而它本来就不该用来做编辑（见开头结论）。
  要对照就**固定 seed 只改一个变量**，别把两件事混在一次里改；
  抽出来的每一张都**如实说清是哪一版**（`seed` 号 + `mediaIds`），让用户自己挑。
- Anima 那份的负面提示词在 cfg=1 时**几乎不起作用**（Turbo 档的物理事实）：
  要排除某个元素就写进**正面**的反面描述，或者提高 cfg 并接受质量变化。
  ⚠ 但**别拿 cfg 去修"贴进来的白框"**：实测 cfg 1.0 与 4.0 的白框一模一样
  （那是 `ImageCompositeMasked` 整块贴像素造成的，不是引导强弱的问题）；
  贴人物图这条路现在压根不走矩形遮罩。
- 用户在手机 / 平板上没法手画遮罩 —— 别让他去 ComfyUI 的 MaskEditor 里描，
  用 `comfy_make_mask` 的框 + 羽化表达意图，或者问他要一组坐标。
