---
name: inpaint-composite
description: 图像编辑的局部重绘工作流 —— 保背景换人物 / 换背景保人物 / 把先生成好的人物贴进某张图。**贴人物图首选 `comfyhub_paste_person_matte_anima.json`**（AI 软遮罩：`easy imageRemBg` 抠出连续 alpha + ring 只融接缝，人物像素与画风原样保留）——⚠ **用之前必须先查 ComfyUI 里有没有抠图模型，缺了要指引用户下载**（下哪个、放哪、多大都在正文；别让它静默下载、也别默默退回颜色抠图）。没有模型时才退到 `comfyhub_paste_person_anima.json` + `comfy_make_mask(mode="subject", ring=N)`（颜色抠图：纯色背景够用，但**会删掉五官**，见正文实测表）。纯重绘 / 换背景保人物用 `comfyhub_inpaint_anima.json`；底图与人物图用 `comfy_use_media` / `comfy_use_attachment` 投进 ComfyUI 的 input。krea2 那两份实测不适合做编辑（重绘不跟随提示词、贴人守不住），别用。遮罩一律"白=重绘区"。
whenToUse: 用户说"保持背景不变，换个人 / 换个动作 / 换身衣服"、"背景换掉但人物不变"、"把这张图里的人换成…"、"先生成人物再放进去"、局部重绘 / inpaint / 遮罩 / 贴人 / 合成人物 / 抠图 / 抠人 / 抠出来时。
version: 3
---

# 图像编辑：局部重绘（保背景换人物 / 换背景保人物）

## ⚠ 结论先看：图像编辑**只用 anima 那份**（`comfyhub_inpaint_anima.json`）

**krea2 那份（`comfyhub_inpaint_krea2.json`）实测不适合做图像编辑** —— 2026-09-26 在活实例上量到的：

| 图像编辑要做的事 | anima | krea2 |
| --- | --- | --- |
| 按提示词重绘遮罩区（纯重绘） | 3 个 seed **每次都跟随**提示词 | 3 个 seed **只有 1 次**跟随；另两次把重绘区里原有的灯柱 / 围墙照旧渲染了一遍 |
| 贴人物图（`20.image` = 阶段一产物） | 🔶 未单独量过（见下） | denoise 0.5 → 人物图那块**带一圈白框**（cfg 1.0 与 4.0 都一样）；denoise 0.85 → 白框没了但**换成了另一个人**（贴进来的人守不住） |

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
| **保背景换人物**（换人 / 换动作 / 换装） | 人物所在位置 | `false` | 底图自己 或 阶段一的人物图 | `1.0`（纯重绘）/ `0.4~0.6`（贴人物图） |
| **换背景保人物** | 人物所在位置 | **`true`** | 底图自己 | `0.85~1.0` |
| 整张重画（等价图生图） | `full` | `false` | 底图自己 | `0.4~0.7` |
| **两阶段**：先生成人物再换上去 | 人物该出现的位置 | `false` | **阶段一产物的 filename** | `0.4~0.6` |

## ★ 先想这一步：**有两条路比抠图更稳**（能用就别抠）

用户说"把生成的人物放进某张背景"时，**先考虑这两条** —— 它们**根本没有边缘问题**，
而抠图（不管多好的模型）永远要在边缘上做取舍：

1. **阶段一就按最终背景生成**（最稳，总纲 §15.3 ①）：
   阶段一**别出"纯色背景的人物"**，而是**拿底图当 i2i 起点**（或同底模 + 提示词里把场景写全），
   阶段二只做 `denoise 0.35~0.5` 的融合重绘 —— 光、色、透视天然一致，画风也是同一个底模。
   ⚠ 代价：**人物姿态会被底图约束**（用户要换大动作就不适合这一条）。
2. **`denoise 0` 纯合成**（一个像素都不改，画风 100% 保留）：
   只靠 alpha 把人物贴上去，配 `ring` 融接缝。**前提是 alpha 要准** —— 所以要么走下面的
   AI 软遮罩，要么背景必须是纯色。⚠ 代价：**光照不融合**（`light` 那一档默认关着，
   观感不合格，**别宣称能用**），近景会显假。

**什么时候才真的需要抠图**：底图/场景已经钉死（比如用户给的实拍照片）、人物要换大动作、
或者要产出"透明人物"素材。那时才走下面这一节。

## 抠图：要"干净的边缘"就**先查模型，缺了让用户下**（贴人物图必读）

贴人物图的成败几乎全在"轮廓抠得干不干净"上。判据只有一条：**要连续 alpha（软遮罩），
不要"删/不删"的硬剪影**。2026-09-27 在真图上量过（768×1024 绿幕动漫人物，尺子 `scripts\matte-probe.ps1`）：

| 抠图方式 | 前景占比 | **软像素占比** | 结果 |
| --- | --- | --- | --- |
| **AI 软遮罩**（`easy imageRemBg`，`rem_mode=BEN2`） | 0.4257 | **3.16%** | 眼睛 / 细发丝 / 发髻都在，边缘是连续 alpha |
| 颜色抠图（`comfy_make_mask(mode="subject")`） | 0.3564 | **0%（硬剪影）** | **把 18.4% 的前景删掉了** —— 眼睛没了，发髻内部与颈部一块块变透明 |

（颜色那条逐档：原始颜色键 43.65% → `drop_blended_edge` 后 **36.30%** → `unmix_edge` 后 35.64%。
根因是颜色键出来的掩码是"麻子"（五官内部本来就有洞），于是"边界 3 像素那条带"**实际盖住大半个主体**，
按颜色扔混合像素就把主体内部整片扔了。**调 `tolerance` 救不回来** —— 调小只是洞小一点、绿边多一点。
详见 `docs/pitfalls.md`。）

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
- ⚠ **别指望 `comfy_env_report` / `comfy_check_inputs` / `comfy_audit_workflow`**：
  它们在当前 Rust 后端**还没下发**，模型看不到（见总纲 §16.5 的欠账）——
  `comfy_get_status` 里的 `mattingModels` 就是为这件事补的。

⚠ **两个坑别踩**（`mattingModels.checked` 已经替你把第一层排掉了，但要知道为什么）：

1. `models\rembg\` 这个**目录名**是 easy-use 写死的（它读 `folder_paths.models_dir` + `rembg`），
   **不是** ComfyUI 原生的 `models\background_removal\`（那是另一套节点 `LoadBackgroundRemovalModel`）；
2. ComfyUI Desktop 把"程序"与"共享数据（input / output）"分在两处，而**模型在安装目录那一份**
   —— 本机实测：`ComfyUI-Installs\ComfyUI\ComfyUI\models\rembg\` 里有两个模型，
   而共享那份 `ComfyUI-Shared\models\rembg\` 是**空的**。所以**只查一处就会误报"缺"**
   （这也是为什么 `checked` 是**每个根各报一行**，不是合并成一句"有/没有"）。
   `comfy_use_media` 返回的 `inputDir` 指向**共享**那一份，**别拿它的兄弟目录当模型目录**。

### ② 缺了就**如实告诉用户下哪个**（别静默下载，也别默默降级）

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

只有一条近似路：`comfy_make_mask(mode="subject", ...)` 的**颜色抠图**，适用面就是上表那一行 ——
**纯色 / 绿幕背景 + 不挑剔边缘**。**含糊不得**：用户要的是"把人干净地抠出来贴到新背景"，
而缺模型时它连五官都会删掉 —— 这种情况**如实说做不到 + 告诉他下哪个模型**，
**绝对不许**硬跑一遍然后说"换好了"。

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
   comfy_load_workflow(path="<项目根>\comfyui\workflows\comfyhub_inpaint_anima.json", includeGraph=true)
     → 拿 promptId（**纯提示词重绘 / 换背景保人物用这份**）
   comfy_load_workflow(path="<项目根>\comfyui\workflows\comfyhub_paste_person_matte_anima.json", includeGraph=true)
     → **贴人物图首选这份**（AI 软遮罩：内部 `easy imageRemBg` 抠 alpha，ring 由 GrowMask+MaskComposite 现算。
       ⚠ 用之前先按上面"抠图"那一节查模型；它**不需要** 12 / 26 两个遮罩覆盖）
   comfy_load_workflow(path="<项目根>\comfyui\workflows\comfyhub_paste_person_anima.json", includeGraph=true)
     → **没有抠图模型时才用这份**（颜色剪影：12 合成用剪影、26 重绘区用 ring）
     （krea2 那两份见开头的结论：不适合做编辑）

   // ★ 首选：AI 软遮罩那份
   // 第 0 步：告诉工具"这一档用工作流的抠图"，拿回 `personFilename`（= 20.image 该填的）与尺寸。
   // ⚠ matteSource="model" 时工具**不写遮罩、不写 ring、也不改人物图一个像素** ——
   //   抠图与**前景估计**都由工作流里的 `easy imageRemBg(refine_foreground=true)` 做：
   //   它用**模型自己的 alpha** 把边界那圈"人 + 绿幕"的混合像素还原成发色。
   //   （我们自己用 Rust 按颜色猜过两版：压通道 → 青边、两侧反混 → 白光晕，都比模型差，已撤掉。
   //    所以**别**再往 Rust 侧加"预处理"的主意 —— 那里没有模型的 alpha，猜不准。）
   comfy_make_mask(mode="subject", matteSource="model",
                   baseFilename="<底图 filename>", subjectFilename="<人物图 filename>",
                   bgColor="green")             // 阶段一用什么背景就传什么（绿幕最稳）
     → 拿 `personFilename` 与 width/height
     → ⚠ 遮罩不要覆盖任何 LoadImageMask（这份工作流没有 12 / 26）

   comfy_submit(promptId=…, overrides={
     "10.image":  "<底图 filename>",
     "20.image":  "<上一步的 personFilename>",   // **清理过**的那一份，不是原图
     "11.width": W, "11.height": H,      // 与底图**必须一致**
     "21.width": W, "21.height": H,
     "15.text":  "<只写**人物外面那一圈**该出现什么 —— 写背景，绝不能写人物>",
     "17.seed":  <换一个新的数字>,
     "17.denoise": 0.4                   // 0 = 纯合成一个像素都不改；0.3~0.45 只融接缝（推荐）
   })

   // 退路：颜色剪影那份（没有抠图模型时）
   comfy_submit(promptId=…, overrides={
     "10.image":  "<底图 filename>",
     "20.image":  "<底图 filename 或 人物图 filename>",
     "12.image":  "<遮罩 filename>",        // 贴人物图时是**剪影**（comfy_make_mask mode=subject）
     "26.image":  "<ring filename>",        // 只有 paste 那份图有：只圈人物外面一圈
     "11.width": W, "11.height": H,      // 与底图**必须一致**
     "21.width": W, "21.height": H,
     "15.text":  "<只写遮罩区里应该出现什么>",
     "17.seed":  <换一个新的数字>,
     "17.denoise": 1.0                   // 贴人物图时改 0.4~0.6
   })
   ```
   ⚠ **第一次用 matte 那份之前**：它的节点名 / 输入名 / 链接都**静态核过**
   （对着本机 ComfyUI 源码 + `scripts\matte-probe.ps1` 的实测），但**还没在活实例上端到端跑过**
   —— 而且当前后端**没有** `comfy_check_inputs` 这类预检工具。所以：第一次跑完把结果
   （成功 / 报错原文）**如实回报**，别假设它一定成；真报错就把原文转述给用户，别自己编原因。
   `easy imageRemBg` 的 `rem_mode` 默认写的是 `BEN2`；想快就覆盖成 `"27.rem_mode": "RMBG-1.4"`，
   ring 的外圈半径是 `"28.expand": 14`（想换更细的接缝就调它）。
   Anima 那份的提示词是 `15.text` + 采样器 `17.*`（**Danbooru tag**，写角色前先按系统提示第 18 条查三处）。
   要是用户坚持用 krea2 那份：提示词节点是 `13.text`、采样器是 `15.*`（**自然语言**，别堆 tag），
   并且**先说清上表那两档风险**（抽卡 / 贴人守不住）。

## 两阶段：先生成人物，再换上去（用户要的正是这个）

1. **阶段一（t2i）**：按角色卡 / danbooru tags 生成人物。要求：
   - 构图与动作**按最终需求**来（反正是要贴进背景的那一个）；
   - 分辨率的长宽比**与底图一致**（省一次拉伸）；
   - 背景：**用与人物身上的颜色明显不同的纯色**（**绿幕 `green` / 品红 `magenta` 最稳**），
     并在 `comfy_make_mask` 里把同一个色传给 `bgColor`（如 `bgColor="green"`）。
     理由（用户原话："应该用不同于画面主体颜色的颜色作为背景"）：**白底 + 白裙子/白衬衫在像素上
     无法区分** —— 抠图只能靠连通性猜，于是"腿间 / 发丝之间"的背景空白会被当成人物内部贴进画面
     （实测看到的就是那一圈白）。绿幕没有这个二义性，判据干净。
     提示词片段：`..., plain solid green background, chroma key green backdrop`。
     ⚠ 返回值里两个字段要**如实转述**：`chromaAdvice`（背景是白/浅色时会出现，说明有二义、建议换绿幕）、
     `bgColorIgnored`（你给的 `bgColor` 与画面里的背景对不上，实际用的是另一个色）。
     ⚠ 背景色**写死也可能不对**（实测：`bgColor=green` 撞上一张模型自己生成、色号不同的绿底图，
     前景占比 100%）—— 工具会自纠（占比 >90% 时回落成"从边框估"），别为此改工作流。
   - 只认返回的 `mediaIds` —— 没拿到产物就别进入阶段二。
2. **阶段二（局部重绘）**：
   - `comfy_use_media(mediaId=<阶段一产物>)` → 人物图的 `filename`；
   - **贴人物图首选 AI 软遮罩**：直接用 `comfyhub_paste_person_matte_anima.json`（**不用**
     `comfy_make_mask`、也不用覆盖 12 / 26）—— 但**先按上面的"抠图"一节查模型在不在**，
     缺了就指引用户下载（别静默下载、也别默默退回颜色抠图）。
   - **没有抠图模型时才用颜色剪影**：`comfy_make_mask(mode="subject", baseFilename=<底图>, subjectFilename=<人物图>, ring=14, feather=…)`
     —— 照人物形状抠（矩形遮罩会把人物图那块矩形连背景一起贴进去 = 一圈白框）；
     `region` 在 subject 档下**只有裁切作用，别传**（默认 full 才是"只按人物形状"）；
     ⚠ 它的代价见上面那张实测表（**五官会被删掉**），只在"纯色背景 + 不挑剔边缘"时够用。
   - **边缘去溢色（despill，自动）**：`mode="subject"` + 彩色背景（绿幕/品红）时，工具会在剪影
     **边界往内 6 像素**那条带上把背景**主色通道**压回另外两个通道的水平（绿幕 → `g=min(g,(r+b)/2)`），
     并把结果**写回同一个文件名**（`20.image` 的覆盖不用改）。返回值里 `despilled=true` 表示动过。
     实测：不去溢色时人物左侧会留**一块绿**、轮廓一圈绿边（羽化让最外几像素半透明，
     而那几像素本身是"人+背景"的混合色）；去了之后干净。
     ⚠ 白/浅灰背景**不触发**（没有"溢色通道"可压）—— 那条路只能靠 `shrink` 与更小的 `feather`。
   - **收边是"按颜色"做的（默认开，`edgeBand=3` / `edgeTolerance=0.35`）**：边界那 3 像素里
     凡是**颜色接近背景色**的像素（"人 + 背景"的混合像素）就扔掉；发丝是头发色、离背景色远，
     **天然保住**。⚠ **别用 `shrink` 去收边**：那是**几何腐蚀**，比 `2*shrink` 细的结构整条消失
     —— **细发丝正好中招**（用户 2026-09-26 指出"细的头发也被你去掉了"），所以 `shrink` 默认 **0**，
     只在"边缘特别脏、按颜色也收不干净"时才手动给 1~2。
   - ⚠ **`ring` 那一圈的提示词必须写"背景"，绝不能写人物**：实测拿人物提示词
     （`1girl, silver hair, white dress…`）去重绘那一圈，模型会在人物周围**又画一圈"白裙子似的东西"**
     → 泛光更重。写背景（`night city street, asphalt, grass, buildings`）才干净。
     但要有心理准备：anima 是**角色模型**，cfg=1 时"no humans"几乎不起作用，那一圈里**可能又冒出一个角色**；
     所以最稳的三档是 ①`denoise 0`（纯合成，不重绘 → 没有泛光也没有乱画）；②ring 细一点（6~10）；
     ③`ring` + 背景提示词 + `denoise 0.3~0.45`。
   - **想让人物一个像素都不被重画（画风不变）就用 `ring`**：它产出"只圈人物外面 N 像素"的
     第二张遮罩，配合 `comfyhub_paste_person_anima.json`（**它有两个 LoadImageMask**）：
     `12.image` ← 剪影（合成用）、`26.image` ← ring（重绘区）。
     实测（2026-09-26）：单遮罩 denoise 0.5 时**人物的裙子被模型换掉了**；
     换成 ring=14 之后**同脸同发色同红裙原样保留**，只有接缝被融掉。
   - **摆放**：`scale`（缩放倍数）+ `x`/`y`（剪影中心放到归一化坐标 0~1）。
     工具会把**人物像素与遮罩一起搬**，并回一个 `placedSubjectFilename` ——
     **必须拿它覆盖 `20.image`**（还用原图就是"人在这儿、遮罩在那儿"的错位）。
   - `denoise`：纯合成（一个像素都不改）= **0**；只融接缝 = 0.3~0.5；整块重画 = 1.0。
     **判据**：要"保住人物的样子/画风"就别超过 0.5，想稳就用 ring。
   - ⚠ **用矩形遮罩（`mode` 默认的 rect）会把人物图那块"像素矩形"整块贴进去**
     （`ImageCompositeMasked` 只认遮罩形状，**不做抠图**）——阶段一那张图的背景会一起进来。
     2026-09-26 实测（白底人物 + 夜景底图，同一组参数只改一个变量）：

     | 遮罩 | denoise | cfg | 结果 |
     | --- | --- | --- | --- |
     | 矩形 | 0.5 | 1.0（工作流原值） | 人物进去了，但**白底变成一圈亮框** |
     | 矩形 | 0.5 | **4.0** | 亮框**照旧**（→ 白框不是 cfg 的锅，别去调它） |
     | 矩形 | **0.85** | 1.0 | 亮框没了，但**人变成了另一个人**（身份/服装守不住） |
     | **剪影** | 0.5 | 1.0 | **白框没了**，但人物被重画（裙子被换掉） |
     | **剪影 + ring 14** | 0.5 | 1.0 | **人物原样保留、只融接缝**（推荐） |

     结论：**白框靠 `mode="subject"` 修；"人物被重画/画风变了"靠 `ring` 修**；
     两者都不是 denoise/cfg 能调的。**边缘要真的干净就上 AI 软遮罩**
     （`comfyhub_paste_person_matte_anima.json`，见上面"抠图"那一节：先查模型、缺了让用户下）；
     没有模型时才用 `mode="subject"`（纯色背景够用，但**会删掉五官** —— 实测掉 18.4% 前景）。
3. **保住画风的四条**（用户的原话是"保持画风"）：
   - 阶段一与阶段二**用同一个底模**（**都 Anima** —— 图像编辑这一档 krea2 不可靠，
     见开头那张表）与同一套 CLIP / VAE；
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
- 要"抠得干干净净"（带 alpha 的透明人物）就上 **AI 软遮罩**（`easy imageRemBg`）：
  **先按上面"抠图"那一节查模型在不在**，缺了就**指引用户下载**（给 URL / 体积 / 目标路径）；
  模型确实没有时**如实说做不到**，别吹，也别拿颜色抠图硬充当"干净"。

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
  （那是 `ImageCompositeMasked` 整块贴像素造成的，不是引导强弱的问题）。
- 用户在手机 / 平板上没法手画遮罩 —— 别让他去 ComfyUI 的 MaskEditor 里描，
  用 `comfy_make_mask` 的框 + 羽化表达意图，或者问他要一组坐标。
