---
name: lora-training
description: 本机 LoRA 训练的参数调优、打标与预处理方法论（Krea-2 与 Anima 两条线）。按"先探环境 → 再定配方 → 再动数据"的顺序，把已有实测经验套到用户这台机器的真实显存与内存上。含：分辨率档 × 宿主内存的口径（8GB 显存是底线，档位看内存）、Krea-2 的当前 target 档位（official_gate 264，attn_mlp 已弃用）与 512 档生产配方、Anima 的 low_vram 配方、宿主内存与 Windows 专属坑（WDDM 换页、pin_memory、cudaMallocAsync）、两种打标口径（Anima 的 Danbooru caption 模板 / Krea-2 的长自然语言 + tag-notes + 确定性闸门）、预处理缓存三件套与"什么变了必须重编"、剂量与"文本通道压过权重"这两个风格 LoRA 的坑、失败模式速查。
whenToUse: 用户要"在本机训 LoRA"、"调显存/内存参数"、"低显存怎么跑"、"这套参数合适吗"、"用哪个 target 档"、"帮我打标/预处理数据集"、"训练 OOM 了/卡住不动/loss 不降"、"风格学不进去"、要挑 rank/lr/步数/分辨率、要把数据集整理成可训练的缓存、要判断某张卡能不能跑某个模型，或者要把训练好的 LoRA 放进 ComfyUI 使用时。
version: 3
---

## 版本记录

- **version 3（2026-10-18，按 krea2-webui 最近四天文档重校）**：这一版**改掉 v1/v2 里的一批错**——
  ① target 档位：**`official_gate` / 264 是当前口径**（2026-10-13 起），不是 `attention`/160；
  `attn_mlp`/196 **已弃用**。② `blocks_to_swap` 的 512 档生产值是 **26**（不是 20）。
  ③ 打标一节整段重写：v1 的"三口径"是编的，真实口径是 Anima 的 Danbooru caption 模板
  与 Krea-2 的"长自然语言 + tag-notes + 确定性闸门"（2026-10-14/15 定稿，见
  `docs/features/dataset-acquisition.md`、`docs/findings/qwen3vl_captioner_confidence.md`）。
  ④ 预处理补上"先聚类去重再下载 / 剔设定稿 / 双通路 sidecar / 512 档必须重跑 resize+latent"。
  ⑤ 新增"剂量"与"文本通道压过权重"两条机制性结论（`krea2_style_repro_20261015_knob_audit.md`）。
- **version 2（2026-10-18，用户口径）**：分辨率档位按**宿主内存**定（16GB→512，24GB→768/1024），
  8GB 显存是"够跑"的底线。
- **version 1（2026-10-18）**：首版。

> **口径纪律**：本 skill 里的数字只有三种来源 —— ① krea2-webui 的 findings / 配置注释（注明日期）；
> ② 本机实测；③ 用户明确给出的口径。**三者之外的一律不许写进来**；拿不准就写"未验证"。

---

## 0. 你能做与不能做

| 能力 | 现状 |
| --- | --- |
| `list_dir` / `read_file` | ✅ 读引擎目录、`output/daemon/*.json`、训练 `stdout.log`、`progress.jsonl`、`configs/*.toml`、数据集清单、`_manifest.json` |
| `write_file` | ✅ 配方 TOML、caption sidecar、给用户跑的脚本、复盘记录 |
| `comfy_*`（comfy-mcp） | ✅ 能问 ComfyUI 实例状态（含显存）与模型清单 —— **本机唯一"不跑命令就能探硬件"的路子** |
| 起训练进程 / 跑 shell | ❌ 没有。**交付物 = 配方 + caption + 可粘贴的命令 + 验收判据** |

**红线**：① 不许撑爆宿主内存（见 §3 闸门）；② 只读源图、只写缓存/产物目录、`output_name` 不许覆盖已有 LoRA；
③ 先 20 步试跑再长跑；④ 起跑后先看 loss 与步时是否"在学、稳"；⑤ 用户的数据集与已有产物一个字不许动。

---

## 1. 先探环境

| # | 要什么 | 哪来 | 判据 |
| --- | --- | --- | --- |
| ① | 显卡 + 显存总量 | `comfy_*` 实例状态；否则问用户 | **8GB 是"够跑"的底线**（有实测） |
| ② | **宿主可用内存** | 训练日志 / 问用户 | < 6GB 不开跑；它决定**分辨率档**（§2） |
| ③ | 引擎装没装 | `train-engine.ps1 status` / `list_dir` 看 `train.py` + `.venv` | 两者都在才算装好 |
| ④ | 数据集 | `list_dir` | 图 + 同名 `.txt` 成对；TE 缓存每张 **31.46MB**，先算磁盘 |
| ⑤ | 底模是哪条线 | 用户 / 引擎配置 | Anima（Qwen3-0.6B TE）还是 Krea-2 Raw（Qwen3-VL-4B TE） |

---

## 2. 分辨率档 × 宿主内存（**档位看内存，不看显存**）

| 显存 | 宿主内存 | 分辨率档 | 依据 |
| --- | --- | --- | --- |
| **8GB** | **16GB** | **512 档** | 块交换要 pinned 宿主副本 + 系统占用；512 档实测 reserved 4.41–4.62GB（显存不是瓶颈） |
| **8GB** | **24GB** | **768 / 1024 档** | 用户本机（4060 Laptop 8GB / 23GB）实测 1024 档可跑 |
| 12–16GB | 32GB+ | 1024 档起步 | 上游默认档口径 |

**显存吃紧的处置顺序**（别一上来砍 rank）：① 开梯度检查点 → ② 加 `blocks_to_swap`（≤ 层数−2，Krea-2/Anima 都是 28 层 → 上限 26）
→ ③ **降分辨率档** → ④ 才考虑 rank/精度。

**512 档的两个硬事实**（2026-10-18 的 512 配置注释）：
* 512 档 **不是精确常量 token**：实测 1008 / 1024 / 1040 / 1050 / 1064 都出现（1024 档才是 4032/4200 两族）；
* `bucket_no_upscale=true` ⇒ 训练侧桶集**直接取自 latent 缓存的 (W,H)**，所以**换档必须重跑
  resize + cache_latents**，不能复用 1024 档的 `.npz`（TE 缓存与分辨率无关，可复用）。

---

## 3. 宿主内存与 Windows 专属坑

| 做法 | 为什么 |
| --- | --- |
| **内存闸门**：可用物理内存 < 6GB 不开跑 | NF4 自包含底模一次要读 6.16GB；块交换再叠 pinned CPU 副本 |
| TE 缓存**按窗口常驻内存**（默认 8 张 ≈ 252MB） | Krea-2 TE 每张 31.46MB：每步从盘读会让步时从 7–8.5s 掉到 11–19s；全塞内存（372 张）又是 11GB+，会顶爆宿主 |
| `dataloader_pin_memory=false` + `persistent_data_loader_workers=false` | Windows 上多进程 + 锁页内存会把宿主内存与句柄顶上去 |
| Krea-2 的 TE 必须 CPU 编 | Qwen3-VL-4B bf16 = **8.27GB** > 8GB 卡的 8.19GB；CPU 实测 ~5.3 s/张（16 核） |
| `PYTORCH_CUDA_ALLOC_CONF=backend:cudaMallocAsync` | **Windows 不支持 `expandable_segments`**，这是替代（实测速度持平、reserved 更低） |
| 别让两个大模型同时驻留 | 上游是"TE → 缓存 → 释放 → VAE → 缓存 → 释放 → DiT"的分阶段加载 |

⚠ **WDDM 换页**：`max_memory_allocated` 超过物理空闲显存**不报错**，只换页 —— 现象是步时
突然 3 倍慢、功耗掉到 40W 上下、风扇安静。按 `reserved` 留余量；关挂在独显上的外接屏/HDR 收益最大。

---

## 4. 打标（两种口径，别再自己发明第三种）

### 4.1 Anima 线：Danbooru caption 模板（对 `datasets/shinku_nte` 的 35 个 sidecar 逐字节验证过）

```text
{trigger}, masterpiece, best quality, {rating},
{主体计数标签}, {版权标签}, {角色标签}, {其余 general 标签}
```

* 来源只有 `tag_string_general` / `tag_string_copyright` / `tag_string_character`；
  **`tag_string_artist` 与 `tag_string_meta` 不参与**（画师身份由 trigger 承载）。
* `rating`：`g→safe`、`s→sensitive`、`q→questionable`、`e→explicit`。
* 主体计数标签（`1girl`/`solo`/`2girls`/`no_humans`…）提到最前；其余 general **按字母序**；
  下划线转空格（`diamond_(shape)` → `diamond (shape)`）。
* `signature` / `watermark` / `dated` / `logo` / `artist_name` / `twitter_username` 在 Danbooru 里
  **属于 general 类**，只丢 meta 去不掉 —— 用可重复的 `--drop-tag` 整批清（只重写 `.txt`，不重下图）。
* `monochrome` / `greyscale` / `sketch` / `lineart` 是**真实内容描述**，不要当噪声删。

### 4.2 Krea-2 线：长自然语言（2026-10-15 用户决策定稿）

**一律长自然语言，句尾不加零散标签。** 推荐档 `full_detail`（3 句 / 70–100 词：姿态与手部、
发色发型、瞳色、每件衣物配饰的颜色、场所物件、天气、光照方向与阴影深浅→**阴影深浅已删，见下**）。

| 做法 | 口径 |
| --- | --- |
| 发色/瞳色、天气、场所、光照 | **全写进句子**；`scene_choice` / `appearance_choice` / `backfill_axis_tags` 降级为**校准与审计信号**，不再渲染进 caption（`apply_nl_captions` **不许**传 `--axis_tags`） |
| 最便宜且最准的路线 | `--notes_tags datasets/<artist>`：拿画师自己的 Danbooru 标签当笔记、只跑一次成文（**13 s/图**）。tag 保发色 213/215、服装 208/215、瞳色 182/215；场所 110/215、天气 81/215、光照 24/215 允许 VLM 看图补 |
| 手部/持物 | 两边都不可靠（98/215）→ 规则**禁止任何 tag 未授权的手部断言** |
| **确定性闸门（必开）** | `--guards hands,mono`（默认开）：4B **服从肯定式、不服从否定式**，所以生成后做**减法** —— tag 没授权手部就删相关小句、单色图删颜色词并补 black-and-white。215 张实测命中 hands 30 张 |
| **色调描述** | **不许写**"光有多强 / 阴影多深 / 对比 / 调色"（2026-10-15 从 `full_detail` 与规则里删掉）—— 它正是"学不到上色"的那个二阶轴；只写**光照方向/来源** |
| 不许写的 | 画风 / 画师名 / 媒介 / 画质词（调性标签走**实测指标 + 冻结阈值**锚定，并剪掉本集退化词：命中 <8% 或 >60%） |
| 触发词 | `@<Artist>` 形式（大小写按 Danbooru 原样） |

⚠ **触发词在 Krea-2 上不是必要条件**：DOP（`diff_output_preservation_class`）**只在 anima 路径实现**，
Krea-2 的 `preprocess_te_cache.py` 没有任何 `prior`/`diff_output` 参数 → `@Artist` 出现在 100% 的
caption 里、是常数，效果被吸进权重。**别承诺"写触发词才生效"**；要让它成为必要条件得新写代码或另加一批不带风格的 class 图。

⚠ **不要拿 perplexity 当打标质量门禁**：实测**错的** caption 困惑度反而更低（1.12 / 1.27 / 1.30）；
自一致性才是可用信号且偏低（token Jaccard 0.392–0.468）。两段式（`--two_pass`，先答 9 条编号问题再成文）
能把漏项补齐并把**逐轴不一致 / 持物自相矛盾**记进 `flags` 供复核，但修不了稳定的系统性错（姿态在三份问卷里一致地错）。

---

## 5. 数据准备与预处理

### 5.1 取图与筛选（先剔再下）

* **先去重再下载**：`fetch_danbooru_dedup` 用 360px 预览聚类，只下"最新的 N 张不同作品"。
  判据两条：预览余弦距离 `< --tight`（0.001，重编码/重传）、Danbooru `parent_id` **直接互链**且距离
  `< --loose`（0.03，裁切/高清重发/叠字）。**不要用互链的传递闭包**（实测 A-B-C 链上 A 与 C 是不同画）。
  实例：`rin_yuu` 719 帖 → 516 张不同作品（**28.23% 重复**）。
* **设定稿要按帖号点名剔除**（三视图/表情集/色板/排版注释教的正是风格 LoRA 最不该学的东西）：
  `--exclude-id` + `--prune`；⚠ 设定稿常以多分辨率重发，**先查 `_dedupe.json` 的 `merged_clusters`
  再定剔除名单**，否则孪生副本会顶上来。
* **分期不要重跑预处理**：`make_dataset_view.py` 用硬链接按 `--since/--until`（上传时间）做视图，
  零磁盘零 GPU；判据用 `pixiv_id` 交叉验证（秩相关 0.89–0.998）。
* ⚠ **不要把数据集塞进 `post_image_dataset/resized` 的子目录**：`base.toml` 的蓝图 `recursive = true`，
  加子目录会**静默改变现有配置的训练集**。

### 5.2 双通路缓存（同一个 cache_dir 同时服务两条线）

sidecar 后缀是隔离的（`_anima_te` / `_krea2_te`），所以不需要两份目录。顺序：
**resize → VAE latents（共用）→ anima TE（可 GPU）→ Krea-2 TE（必须 CPU）**。

| 产物 | 单张体积 | 备注 |
| --- | --- | --- |
| `{stem}_{WxH}_anima.npz`（latent） | ~1.0MB | **两通路共用**；与分辨率绑定 |
| `{stem}_anima_te.safetensors` | ~1.0MB | anima 通路 |
| `{stem}_krea2_te.safetensors` | **31.46MB** | `hiddens[512,12,2560]` bf16 + mask → 1000 张 ≈ **31GB**，2000 张 60GB+ |

* `resize_images.py` 默认 `--min_pixels 500000`，会把 <0.5MP 的原图挡在 resized 外；要带上就传
  `--min_pixels 0`，然后**重跑 resize + 两个 TE 缓存**（脚本幂等，只补缺的）。
* **换档（512↔1024）必须重跑 resize + latent**；**改 caption / 换触发词必须重编 TE**
  （latent 不受影响）。
* ⚠ **E: 盘写入按解释器分**：仓库 `.venv\Scripts\python.exe`（预处理/TE 用的就是它）写 `E:` 会
  `PermissionError [Errno 13]`，safetensors 走 rust 写入时报的是 `os error 5` —— **别误判成显存/路径/模型问题**。
  做法：**先写工作区暂存目录，再用 pwsh 搬**。用 DSH 自带 runtime 的 python 做预检会得到**假阳性**。

---

## 6. 配方（两条线分开）

### 6.1 Krea-2 Raw（当前口径，逐键对齐 512 档生产配置）

| 键 | 值 | 说明 |
| --- | --- | --- |
| `model_family` | `krea2_raw` | 与 Anima 同仓不同 line |
| 底模 | **NF4 自包含**（`base_compute="nf4"`） | bf16 26GB 在 8GB 卡上不可能加载 |
| `krea2_target_preset` | **`official_gate` = 264** | = 官方 232 + 32 个 `mlp.gate`（**2026-10-13 起的口径**）；r16 → **58.65M** 参数。gate 与 `mlp.up` 同形状、**不引入新张量宽度**，所以 8GB 能跑；官方/第三方在用的风格 LoRA 有 **17.7–26.4%** 的 delta 能量落在这个 gate 上 |
| 其它档 | `official`=232（47.95M，8GB 上 1024 档会越线换页）、`attention`=160（25.03M，窄档） | **`attn_mlp`=196 已弃用**，别再选 |
| `network_dim` / `network_alpha` | 16 / 16 | α=dim 是满强度 |
| `learning_rate` / `lr_scheduler` | **3e-4** / constant、warmup 0 | 2026-10-15 起从 1e-4 上调（官方区间偏保守端） |
| `timestep_sampling` | `sigma` | + `discrete_flow_shift=1.0` + `weighting_scheme="uniform"` |
| `train_modulation` | **false** | 2026-10-09 用户决策：一律不开（开了那个键进不了 ComfyUI 原生路径） |
| `blocks_to_swap` | **26** | 512 档生产值 |
| 回调/编译 | `gradient_checkpointing=true`、`selective_checkpoint="off"`、`attn_mode="torch"`、`torch_compile=false` | 8GB 上 compile 方向为负 |
| 数据侧 | `dataloader_pin_memory=false`、`persistent_data_loader_workers=false`、`cache_text_encoder_outputs=true`、`caption_dropout_rate=0.0` | caption dropout 路线实测是空结果（2026-10-15） |

**实测（同一台 4060 Laptop 8GB / 23GB 宿主）**

| 来源 | 配置 | 步时 | 显存 | 说明 |
| --- | --- | --- | --- | --- |
| krea2-webui 生产 | 512 / official_gate / swap26 / 全梯度检查点 | **7.06–8.55 s/step** | `reserved` **4.41–4.62GB** | 1860 步 = 372 张 × **5.0 遍** ≈ 3.6h；87W / 68°C |
| 本仓下游 line（`anima-lora-next`） | 同上（最小编织，无预取流水线） | **11.7–19.3 s/step** | 峰值 **4.95GB** | 264 target / **58,652,880** 参数 / 产物 223.8MB；比生产慢 ~1.5–2×，差在数据预取 |

### 6.2 Anima

`preset=low_vram`（梯度检查点 + Unsloth 卸载检查点）+ `blocks_to_swap=0`；本机实测 896×1200
（4200 tokens）单桶、160 步 9m37s ≈ **3.6 s/it**、GPU 峰值 **7160 MiB / 8188**。
三条硬约束：`unsloth_offload_checkpointing` 与 `blocks_to_swap` **互斥**；交换数 ≤ 层数−2；
Windows 不支持 `expandable_segments`。Anima 的默认值会咬人：`vocab_pack`（CJK 词表包）**默认开**、
`channel_scaling_alpha=0.5` 默认开、`train_adaln=true`、`down_init="weight_svd"`、
`use_repa=true`（**要 PE 缓存，缺了直接报错**，这是好事）；分辨率是 free-fit 原生形状
（`EDGE_TOKEN_BANDS` + `compile_dynamic_seq`），`target_res` **只在预处理阶段有用** ——
老配方里的 `enable_bucket` / `min_bucket_reso` 那套在 Anima 线上不存在。

### 6.3 步数（**剂量**比"跑多久"重要）

实测（264 模块口径的 ‖ΔW‖）：用户正在用的参照 LoRA `nai_ba_style_krea2-st6000` = **1511.8**；
本仓 1000 步的一臂只有 **470.1**（31%）、430 步的 **256.1**（17%）。**"欠拟合"是可以量出来的**：
风格学不进去时先看剂量（步数 × 数据集遍数），而不是先换 rank。512 档的实践口径是
**1860 步 = 372 张 × 5 遍**。经验值：35 张 → 1000–2000 步；700 张 → 1500 步上下；
先 20 步确认在学，再按"遍数"放大。

**机制性结论（2026-10-15 审计，最值钱的一条）**：**文本通道压过权重** —— 训练 caption 里把
打光/上色写成**自由文本**，推理时一写它，控制权就交回底模先验（只加一段
`Coloring and lighting: …` 的效应量 ≥ 换掉整条 prompt）。所以：**captions 里不许出现色调描述**。
同批还证否了两条：caption dropout **不是**让画风 on-load 的路子；触发词是效应量**最小**的旋钮。

---

## 7. 失败模式速查

| 症状 | 真因 | 处置 |
| --- | --- | --- |
| 第一步就 `CUDA out of memory` | 激活内存 | 梯度检查点 → 加 swap → **降档** → 才动 rank |
| 步时突然慢 3 倍、功耗掉到 40W、不报错 | **WDDM 换页** | 按 `reserved` 留余量；关外接屏/HDR |
| `Expected all tensors to be on the same device, cuda:0 and cpu`（只在开 swap 时） | **adapter 嵌在 block 里**：块交换按名字成对搬 block 的 `.weight` | LoRA 参数放到 **block 外**（functional 注入 / holder 挂模型根下）——上游 `qwen21`/`moss` 的 CLAUDE.md 都写了这条 |
| `Cannot swap more than N blocks` | 交换数 ≥ 层数−2 | 降到 26（28 层模型） |
| `unsloth_offload_checkpointing` 与 swap 一起报错 | 两个省显存手段互斥 | 二选一 |
| `use_repa is enabled but ... no PE feature cache` | REPA 默认开但没做 PE 缓存 | 补 `preprocess-pe-spatial`，或**显式** `use_repa=false` |
| 步时比预期慢一倍 | TE 缓存每步从磁盘读（每张 31.46MB） | 让 TE 缓存**按窗口常驻内存** |
| 换了分辨率档，某些图被静默跳过 | 桶集取自 latent 的 (W,H)，旧档 `.npz` 对不上 | **重跑 resize + cache_latents**（TE 可复用） |
| 训练"在跑"但画风没进去 | ① 剂量不够（先算 ‖ΔW‖）② caption 把打光/上色写成了自由文本 ③ 改了 caption 没重编 TE | 见 §6.3 与 §5.2 |
| 打标出来的 caption 有"hands on hips"这类幻觉 | 4B 服从肯定式不服从否定式 | `--guards hands,mono`（默认开，必开） |
| `PermissionError` / `os error 5` 写不进 E: | 写入权限按**解释器**分（仓库 `.venv` 不行） | 先写工作区，再 pwsh 搬 |
| 只有声音没有画面（视频） | 与本 skill 无关：Impeller 下外部纹理 0×0 | 见 AGENTS §9 的 `third_party/video_player_win` 补丁 |

---

## 8. 在 ComfyHub 里怎么落地

- 引擎（可选能力）：`pwsh -File scripts\train-engine.ps1 status|start|stop|open`，
  默认 `<根>\training\anima_lora`，**建议装在工作区外**。它是上游 `sorryhyun/anima_lora` 的
  **下游整合版**（中文补丁 + Krea-2 内核，见 `docs/training-engine.md`）。
- 对外只有一条面：`anima_daemon` 的 localhost REST（`POST /jobs`、`GET /jobs/{id}/progress`…），
  端口只从 `<引擎>\output\daemon\daemon.json` 读。
- 产物：`<引擎>\output\ckpt\**\*.safetensors`，元数据带 `ss_adaln_layout=comfy`（Anima）/
  `modelspec.architecture=krea2-raw/lora`（Krea-2）→ **可直接复制进 ComfyUI 的 `models\loras\`** 用。
- 你（AI）现在的交付物：配方 TOML + caption sidecar + 可粘贴的命令 + 验收判据；
  等 ComfyHub 的 `train` 域上线后才轮到"直接提交作业 / 看曲线"。

## 9. 一页速查

```
1) 显存 ≥ 8GB 就能跑；档位看宿主内存：16GB → 512 档，24GB → 768/1024 档
2) 可用内存 < 6GB → 不开跑
3) 数据：先聚类去重（别用互链传递闭包）→ 剔设定稿 → 图 + 同名 .txt 成对
4) 打标：Anima = Danbooru 模板；Krea-2 = 长自然语言 full_detail 或 tag-notes + --guards hands,mono；
   captions 里不许写色调描述、不许写画风/画质词
5) 预处理：resize → VAE latent（两通路共用）→ anima TE（可 GPU）→ krea2 TE（**必须 CPU**）
   换档重跑前两步；改 caption 重编 TE；E: 盘先写工作区再搬
6) 配方：Krea-2 = official_gate 264 / r16 α16 / lr 3e-4 constant / sigma+flow1.0+uniform /
   swap26 + 全梯度检查点 / attn_mode=torch / 不开 compile
7) 先 20 步：看 loss、看 reserved（≤ 卡容量）、看步时稳不稳（512 档 ~7–8.5s 是生产值）
8) 剂量：风格学不进去先看步数 × 遍数（参照 ‖ΔW‖ 1511.8 vs 1000 步 470）；512 档实践 5 遍
9) 产物进 ComfyUI 的 models\loras\ 验证
```
