---
name: lora-training
description: 本机 LoRA 训练的参数调优、打标与预处理方法论。按"先探环境 → 再定配方 → 再动数据"的顺序，把已有实测经验（本机 4060 Laptop 8GB / 23GB 宿主、Anima 与 Krea-2 两条线的实测数字）套到用户这台机器的真实显存与内存上。含：显存/宿主内存的档位表与 Windows 专属坑（WDDM 换页、pin_memory、alloc conf）、三种打标口径（Anima 的 Danbooru tag 栈 / Krea-2 的 NL 长句 / tag-notes 混合）+ 触发词与闸门、预处理缓存三件套与"什么变了必须重编"、Anima 与 Krea-2 的配方决策树、失败模式速查（症状→真因→处置）、以及训练引擎装在哪、产物怎么进 ComfyUI。
whenToUse: 用户要"在本机训 LoRA"、"调显存/内存参数"、"低显存怎么跑"、"这套参数合适吗"、"帮我打标/预处理数据集"、"训练 OOM 了/只有声音/卡住不动"、要挑 rank/lr/步数/分辨率、要把数据集整理成可训练的缓存、要判断某张卡能不能跑某个模型，或者要把训练好的 LoRA 放进 ComfyUI 使用时。
version: 2
---

## 版本记录

- **version 2（2026-10-18，用户口径）**：**分辨率档位按宿主内存定，不按显存** ——
  8GB 显存是"够跑"的底线；**16GB 内存 → 512 档**，**24GB 内存 → 768/1024 档**
  （用户本机 4060 Laptop 8GB / 23GB 实测 1024 档可以跑）。§2 与 §9 已按此改写；
  显存不够时的处置顺序也钉死为"梯度检查点 → 块交换 → 降档 → 才动 rank"。
- **version 1（2026-10-18）**：首版。素材来自三处实测：① `krea2-webui` 的 findings/配置注释（8GB 卡上
  Krea-2 的 NF4/swap/checkpoint/compile 一大批边界）；② 本仓库这次把上游 `anima_lora` 装进
  `E:\anima_lora` 并跑通 20 步 Anima LoRA 的实测（`docs/training-engine.md`）；
  ③ 下游整合版 `anima-lora-next` 在 512 档跑通 20 步 Krea-2 LoRA 的实测。
  ⚠ 本 skill 只写"已经有人实测过"的数字；没实测过的一律标"未验证"。

---

## 0. 你能做与不能做（先说清，免得给用户空头支票）

| 能力 | 现状 |
| --- | --- |
| `list_dir` / `read_file` | ✅ 可以读引擎目录、`output/daemon/*.json`、训练 `stdout.log`、`progress.jsonl`、`configs/*.toml`、数据集目录清单 |
| `write_file` | ✅ 可以写数据集配置、caption sidecar（`{stem}.txt`）、训练配方 TOML、给用户跑的脚本 |
| `comfy_*` 工具（comfy-mcp） | ✅ 其中能问 ComfyUI 的实例状态（含显卡/显存）与模型清单 —— **这是本机唯一"不用跑命令就能探测硬件"的路子** |
| 起训练进程 / 跑 shell | ❌ 没有这条工具。**所以你的交付物是**：① 一份可执行的配方（TOML/命令）② 一句话告诉用户在哪跑 ③ 验收判据（哪一行出现算成功、什么算失败） |

**红线（这套里最容易出事的地方）**：
1. **不许让用户的机器内存爆掉**。他自己还要用电脑 —— 见 §3 的"内存闸门"，可用内存 < 6GB 不开跑。
2. **不许动用户的数据集与已有产物**：只读源图、只在缓存目录里写、`output_name` 不许覆盖已有 LoRA。
3. **先 20 步试跑再长跑**：通路/参数问题的代价从"一夜"降到"十分钟"。
4. 训练跑起来之后**先看 loss 与步时**再让它长跑；"能起"不等于"在学"。

---

## 1. 第一步永远是探环境（不要凭"用户说了 8GB"就开配方）

要凑齐这五个数，其中 ①④ 你可能拿不到 —— 那就**明确告诉用户"我需要这两个数"**，别猜：

| # | 要什么 | 哪来 | 判据 |
| --- | --- | --- | --- |
| ① | 显卡型号 + **显存总量** | `comfy_*` 的实例状态（够用时）；否则问用户 | 8G / 12G / 16G / 24G 是分档线 |
| ② | **宿主可用内存** | 读训练日志；否则问用户 | < 6GB 不要开跑；16GB 是"能不开 swap 训练"的线 |
| ③ | 引擎装没装、装在哪 | `train-engine.ps1 status` 的结论 / 读 `<根>\training\anima_lora\train.py` 是否存在 | 目录里有 `train.py` + `.venv` |
| ④ | 驱动 / CUDA | 引擎 `install.ps1` 的日志（它会打印） | 上游要求 Ampere 以上 + CUDA 13.2 toolkit |
| ⑤ | 数据集在哪、有多少张、有没有 `.txt` | `list_dir` 扫目录 | 成对（图 + 同名 `.txt`）才进得了训练 |

**判断顺序**：显存 → 定"能不能跑 + 什么精度"；宿主内存 → 定"能不能开块交换"；数据集 → 定"打标要不要重做"。

---

## 2. 分辨率档位表（**8GB 显存是底线；档位由宿主内存定** ← 用户本机口径）

**先记住这条**：显存 8GB 就够跑训练了；**决定用哪一档的是宿主内存**，不是显存。

| 显存 | 宿主内存 | 分辨率档 | 说明 |
| --- | --- | --- | --- |
| **8GB** | **16GB** | **512 档**（如 448×576 ≈ 1008 tokens） | 块交换的 pinned 副本要吃掉 5~6GB 宿主；16GB 只够 512 档 |
| **8GB** | **24GB** | **768 / 1024 档** | 用户本机（4060 Laptop 8GB / 23GB）实测 **1024 档可以跑**；24GB 有富余给 pinned 块与系统 |
| 12–16GB | 32GB+ | 1024 档起步，可上 free-fit 多档 | 上游默认档口径 |
| 24GB+ | 32GB+ | 多档 free-fit + 更大 rank | 上游默认档口径 |

**显存不够时的处置顺序**（别一上来砍 rank —— 实测 rank 16→8 只省 145MB、步时不变）：
① 开梯度检查点 → ② 加 `blocks_to_swap`（≤ 层数-2）→ ③ **降分辨率档** → ④ 才考虑 rank/精度。

| 卡档 | 推荐配方 | 实测/依据 |
| --- | --- | --- |
| **8GB（RTX 4060 Laptop 实测）** | `preset=low_vram`（= 梯度检查点 + Unsloth 卸载检查点）+ `blocks_to_swap=0`；896×1200（4200 tokens）单桶；不开 `torch_compile` 也能跑 | 160 步 9m37s ≈ **3.6 s/it**，GPU 峰值 **7160 MiB / 8188**，loss 从 0.25 降到 0.13 量级 |
| 12–16GB | 默认档（无卸载检查点）；可开 `torch_compile` + free-fit 多档 | 上游文档口径 |
| 24GB+ | 默认档 + 多分辨率档 + 更大 rank | 上游文档口径 |

**Krea-2 侧的同一件事**（NF4 自包含 + 块交换，实测）：512 档 = **5.36GB 峰值 / ~10–15s per step**；
分辨率往上走时先看宿主内存还剩多少 —— 块交换的块越多、pinned 内存越大。

**三条硬约束（踩了就报错，不是变慢）**：
- `unsloth_offload_checkpointing` 与 `blocks_to_swap` **互斥**（上游 assert：`blocks_to_swap is not supported with unsloth_offload_checkpointing`）。
- 块交换最多 `num_blocks - 2`（28 层的 Anima/Krea-2 → 上限 26）。
- **Windows 不支持 `expandable_segments`**，用 `PYTORCH_CUDA_ALLOC_CONF=backend:cudaMallocAsync` 替代（实测速度持平、reserved 更低）。

**Windows/WDDM 专属坑（最坑的一条）**：`max_memory_allocated` 超过物理空闲显存时**不报错**，
而是被 WDDM 换页 —— 现象是步时突然 3 倍慢、功耗掉到 40W 上下、风扇也安静了。
处置：按 `reserved`（不是 `allocated`）留余量；关掉挂在独显上的外接显示器/HDR 是单点收益最大的一步。

**Anima 的默认值会咬人**（fork 里的配方没有这些，搬过来结果就不一样）：
`vocab_pack`（CJK 词表包，v2 起**默认开**）、`channel_scaling_alpha=0.5`（默认开）、
`train_adaln=true`/`adaln_rank=16`、`down_init="weight_svd"`、`use_repa=true`（**默认开，且要 PE 缓存**）。
分辨率分桶是 **free-fit 原生形状**（`EDGE_TOKEN_BANDS`，必须 `compile_dynamic_seq`），
`target_res` **只在预处理阶段有用** —— 老配方里那套 `min_bucket_reso/max_bucket_reso/enable_bucket` 在这条线上不存在。

---

## 3. 宿主内存（比显存更容易翻车）

| 做法 | 为什么 |
| --- | --- |
| **内存闸门**：可用物理内存 < 6GB 不开跑（探针/脚本里就有这个闸门） | 加载 NF4 自包含底模要一口气读 6.16GB；再叠加块交换的 pinned CPU 副本 |
| 块交换的代价要如实说 | 20 块 pinned 在 CPU ≈ 6GB 宿主；好处是显存从"装不下"降到 3.8~5.4GB |
| `dataloader_pin_memory=false` + `persistent_data_loader_workers=false` | Windows 上多进程 + 锁页内存会把宿主内存与句柄都顶上去 |
| TE（文本编码器）放 CPU 编 | Krea-2 的 Qwen3-VL-4B bf16 是 **8.9GB**，8GB 卡塞不下 → 必须 `--device cpu` |
| 别让两个大模型同时驻留 | 上游是"TE → 缓存 → 释放 → VAE → 缓存 → 释放 → DiT"的分阶段加载，别自己改成并行 |

---

## 4. 打标（三种口径，选错等于白训）

| 口径 | 用在哪 | 长什么样 | 要点 |
| --- | --- | --- | --- |
| **Danbooru tag 栈** | Anima 主力 | `1girl, solo, @artist, ...` | 触发词用 `@artist` 形式；上游有 `caption_index`/tag KB 生态；`anime_tools` 的 autotag/position/OCR 三个阶段 + danbooru 标签库 |
| **NL 长句（Qwen3-VL 打标）** | Krea-2 / 需要语义理解 | `hiten_mono_style, One person ... holding a towel, in a dimly lit room.` | krea2-webui 用 `scripts/dataset/qwen3vl_caption_dir.py`，带 `--guards hands,mono` 这类**确定性闸门**、`--resume` 断点续跑 |
| **tag-notes 混合** | 风格 LoRA（画风） | 触发词 + 画师自己的标签当"笔记" + 句子 | 最新口径：触发词 `@Artist`（与 `@Twinbox`/`@fajyobore` 同口径），句子由 captioner 出 |

**打标的四条纪律**：
1. **触发词要唯一且不与底模已有概念冲突**（`hiten_mono_style` → 后来改成 `@hiten` 这种形式，是因为要跟其它画师 LoRA 保持一致）。
2. **闸门（guards）比提示词更值钱**：`hands, mono` 这种"确定性判据"能把打标跑偏的一批图挑出来，比事后肉眼翻一遍可靠。
3. **caption 一改，TE 缓存必须重编**（`_krea2_te` / `_anima_te` 与 caption 绑定；VAE latent 与 caption 无关、但**与分辨率绑定**）。
4. **别在 ComfyHub 里直接改用户的源图目录**：写 sidecar 可以，删/改名不行。

---

## 5. 预处理（缓存三件套 + 什么时候要重做）

| 步骤 | 产物 | 什么时候必须重做 |
| --- | --- | --- |
| resize（分桶缩放） | `post_image_dataset/resized/**` | 换分辨率档 / 改裁剪策略 |
| VAE latent 缓存 | `{stem}_{W}x{H}_anima.npz` | **换分辨率**（文件名的 WxH 就是判据）；σ 降档训练还要 demote 兄弟 |
| TE 缓存 | `{stem}_anima_te.safetensors` / `{stem}_krea2_te.safetensors` | **caption 变了**、换了 TE、开了/关了 CJK 词表包 |
| PE 特征（REPA 用） | `{stem}_anima_pe_spatial.safetensors` | `use_repa=true` 但缓存缺失时会**直接报错**（上游 fail-closed，这是好事） |

**8GB 卡上的预处理顺序**：resize（CPU/GPU 都行）→ VAE cache（GPU，20 张约 13s）→ TE cache（**CPU**）→ PE（用了 REPA 才做）。
实测：20 张图 896×1200 全链路一两分钟量级。

⚠ **缓存与"档位"的关系**：Krea-2 这条线在 512 档是 448×576（**1008 tokens**）这种桶；
Anima 这条线是 free-fit（同样 896×1200 → **4200 tokens**）。混用两档的 latent 会出现"缺桶"，
表现为训练时某些图被静默跳过 —— **换档就重跑 resize + VAE cache**，TE 缓存可以复用（与分辨率无关）。

---

## 6. 配方决策树（Anima / Krea-2 两条线分开）

**先问一句：哪个底模？**
- **Anima**（`anima-base-v1.0` + Qwen3-0.6B TE + Qwen image VAE，约 5.5GB）→ 走引擎的
  `preset=low_vram` 那条链；配置键就是 `configs/base.toml` 那套（见 §2 的"默认值会咬人"）。
- **Krea-2 Raw** → **必须 NF4 自包含 v2**（bf16 26GB 在 8GB 卡上不可能加载）；512 档实测：
  `rank=16/alpha=16`、`lr=3e-4 constant`、`target_preset=attention`（160 个 target / **25.03M** 参数）、
  `blocks_to_swap=20` + 全程梯度检查点、`attn_mode=torch`、不开 compile →
  **~10–15s/step，显存峰值 5.36GB**。

**步数/rank 的经验值**（都来自实测，不是理论值）：
- 数据 35 张 → 1000~2000 步量级；700 张 → 1500 步量级；**先跑 20 步确认在学**再放大。
- 风格 LoRA：rank 16~32 起步；`alpha=rank` 是"满强度"（ComfyUI 权重 1.0 就是 1.0），
  `alpha < rank`（如 16/32）等于默认打 0.5 折，别在训练时打折、要在推理时调权重。
- Krea-2 侧实测：rank 16→8 只省 145MB 显存、步时不变（**别把 rank 当速度旋钮**）。

---

## 7. 失败模式速查（症状 → 真因 → 处置）

| 症状 | 真因 | 处置 |
| --- | --- | --- |
| `CUDA out of memory` 第一步就炸 | 激活内存，不一定是权重 | ① 开梯度检查点 ② 加 `blocks_to_swap` ③ 降分辨率档 ④ 才考虑减 rank |
| 步时突然慢 3 倍、功耗掉到 40W、不报错 | **WDDM 换页**（超了物理显存） | 按 `reserved` 留余量；关外接屏/HDR |
| `Expected all tensors to be on the same device, cuda:0 and cpu`（只在开块交换时） | **adapter 嵌在 block 里** —— 块交换成对搬 block 的 `.weight` | LoRA 参数放到 **block 外**（functional 注入 / holder 挂在模型根下） |
| `Cannot swap more than N blocks` | 交换数 ≥ 层数-2 | 降到 ≤26（28 层模型） |
| `blocks_to_swap is not supported with unsloth_offload_checkpointing` | 两个省显存手段互斥 | 二选一：要卸载检查点就 `swap=0` |
| `use_repa is enabled but none of the N training images have a ... PE feature cache` | REPA 默认开但没做 PE 缓存 | 要么补 `preprocess-pe-spatial`，要么显式 `use_repa=false`（**别静默**） |
| 训练"在跑"但 loss 不动 / 出图没变化 | caption 改了没重编 TE；或触发词与底模冲突 | 重编 TE 缓存；检查触发词 |
| 加载底模时宿主内存被吃光 | 自包含 NF4 要一次性读 6GB+ | 先满足 §3 的内存闸门；关掉其它大户 |
| 只有声音没有画面（视频） | **与本 skill 无关**：Impeller 下外部纹理 0×0，见 AGENTS §9 的 `third_party/video_player_win` 补丁 | 别顺手升级回 pub.dev 版本 |

---

## 8. 在 ComfyHub 里怎么落地（现实状况）

- **引擎**（可选能力）：`pwsh -File scripts\train-engine.ps1 status|start|stop|open`，
  默认装在 `<根>\training\anima_lora`，**建议装在工作区外**（§12 的 Low 标签问题）。
  它是上游 `sorryhyun/anima_lora` 的**下游整合版**（含中文补丁 + Krea-2 内核）。
- **引擎的对外面只有一条**：`anima_daemon` 的 localhost REST（`POST /jobs`、`GET /jobs/{id}/progress`…），
  端口只从 `<引擎>\output\daemon\daemon.json` 读。
- **产物**：`<引擎>\output\ckpt\**\*.safetensors`，元数据带 `ss_adaln_layout=comfy` / Krea-2 的
  `modelspec.architecture=krea2-raw/lora` → **可以直接放进 ComfyUI 的 `models\loras\`** 用 LoRA Loader 加载。
  给用户的建议是"复制过去"，而不是让 AI 去写一套加载器。
- **你（AI）现在的交付物**：配置 TOML + caption sidecar + 一份可直接粘贴的命令 + 验收判据。
  等 ComfyHub 的 `train` 域上线后，才轮到"直接提交作业/看曲线"。

## 9. 一页速查：给一台新机器开配方

```
1) 显存多少？ 8G 就够跑（底线）；< 8G 才需要考虑更激进的省显存手段
2) 宿主内存多少？ 16G → 512 档；24G → 768/1024 档（**档位看内存，不看显存**）
                可用内存 < 6GB → 不要开跑，先让用户清内存
3) 数据成对吗（图 + 同名 .txt）？→ 不成对就先打标
4) caption 口径：Anima=tag 栈 / Krea-2=NL 句子 / 风格=触发词+tag-notes
5) 预处理：resize（按档）→ VAE cache（换档必重做）→ TE cache（改 caption 必重做）→ PE（用 REPA 才做）
6) 先 20 步：看 loss 降不降、看 reserved 有没有越线、看步时稳不稳
7) 再放大步数；产物落 output/ckpt，复制进 ComfyUI 的 loras 验证
```
