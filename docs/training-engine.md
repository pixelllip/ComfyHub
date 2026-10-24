# 训练引擎（可选能力）—— 现状、脚本、以及"接进 App"的落点

> 状态：**全链路已接通**（2026-10-21）。脚本层 + Rust `train` 域 + Flutter 训练台
> （内嵌 WebUI）+ 训练引擎的 MCP 桥都已经落地并实测。
> 一句话：训练引擎是**可选能力**，按 AGENTS §8 的纪律（检测优先、安装兜底、装不上不拦启动）走。
>
> 2026-10-21 起，WebUI **并回下游仓库**（`anima-lora-next/anima_lora_webui`）是唯一真源；
> 兄弟目录 `anima-lora-webui` 只是历史副本，`train-engine.ps1` 与 Rust 都会**先找引擎自带的**。

## 1. 引擎是什么

| 仓库 | 内容 | 备注 |
| --- | --- | --- |
| `pixelllip/anima-lora-next` | 上游 `sorryhyun/anima_lora` 的**下游整合版**：训练/推理/守护进程内核 + 中文界面补丁 + Krea-2 内核 | 以上游为基线，带"每日跟版"CI（[DOWNSTREAM.md](https://github.com/pixelllip/anima-lora-next/blob/main/DOWNSTREAM.md)） |
| `anima-lora-next/anima_lora_webui` | 自有 Web 界面（标准库，零依赖，常驻 ~25MB） | 2026-10-18 起**并回下游仓库**；兄弟仓库 `pixelllip/anima-lora-webui` 只保留历史 |

引擎的对外面只有一个：**`anima_daemon` 的 localhost REST**（JSON，无 CORS 头）：

```
GET  /health                    守护进程健康 + pid/port/root
POST /jobs                      {method,preset,methods_subdir,overrides,extra,start} 或 {kind:"command",argv,label}
GET  /jobs                      作业列表（含 state/rc/overrides/result_summary）
GET  /jobs/{id}                 单个作业 + latest/stale_for
GET  /jobs/{id}/progress?last_n=2000   结构化事件（ev=step|ckpt|log|run_start|run_end）
GET  /jobs/{id}/logs            stdout+stderr 的 SSE
POST /jobs/{id}/{stop,pause,resume} · /queue/{start,pause} · /shutdown
GET  /tools                    自描述清单（带 JSON-Schema；2026-10-21 起含 14 个 /studio 工具）
```

**训练工作区（2026-10-21 新增，`anima_daemon/studio.py`）**：

```
GET  /studio/projects                          项目列表（含五阶段状态）
POST /studio/projects                          {name,trigger?,variant?,preset?,...}
GET  /studio/projects/{slug}                   项目详情 + 阶段→作业账本
POST /studio/projects/{slug}/import            取训练集（本机路径 / http(s) URL → inbox）
GET  /studio/projects/{slug}/images?state=     图片 + caption + 缩略图 URL
POST /studio/projects/{slug}/review            人工筛选 accept/reject/restore/delete
GET/POST /studio/projects/{slug}/caption       读 / 写 caption sidecar
POST /studio/projects/{slug}/trigger           给全部已接受图补触发词
POST /studio/projects/{slug}/tag               打标作业（caption-autotag）
POST /studio/projects/{slug}/preprocess        预处理作业（preprocess-config；可 chain_train）
POST /studio/projects/{slug}/train             正式训练（生成 scoped 配置并提交）
GET  /studio/projects/{slug}/file?path=        图片字节（WebUI 缩略图）
```

数据落在引擎目录里，**没有任何一份散到 `%APPDATA%`**：

```
training_projects/<slug>/{project.json,inbox/,rejected/,jobs.json,train_config.toml}
image_dataset/<slug>/                       已接受的图 + 同名 .txt
post_image_dataset/resized/<slug>/          预处理派生
post_image_dataset/lora/<slug>/             VAE/TE 缓存
output/ckpt/<slug>/                         训练产物
```

路径之所以是 `image_dataset/<slug>`：引擎的 Qt GUI 本来就有一个 GUI-only 的
`path_scope`，会把 `image_dataset/<scope>` → `resized/<scope>` → `lora/<scope>` →
`output/ckpt/<scope>` 一次性展开。复用它，训练器 / 预处理 / mask / ComfyUI trainer node
就都同意同一套路径，**不用写第二份路径推导**。

端口的唯一真源是 pidfile：`<引擎>\output\daemon\daemon.json`（还有一份用户级镜像
`%USERPROFILE%\.anima\daemon.json`，可用 `ANIMA_DAEMON_PIDFILE` 覆盖 —— 见 §5 的一条纪律）。

## 2. 脚本：`scripts\train-engine.ps1`

| 动作 | 干什么 |
| --- | --- |
| `status`（默认） | 只看：引擎在不在（`train.py` + `.venv`）、守护进程起没起（按 pidfile 探 `/health`）、WebUI 起没起。**不联网、不起进程** |
| `start` | 起守护进程 + WebUI（`pythonw` + `-WindowStyle Hidden`，无控制台窗口），把 pid/端口写进 `<根>\.run\train-engine.json` |
| `stop` | 关掉 `start` 起的那些（WebUI 按 state 里的 pid；守护进程走它自己的 `POST /shutdown` 契约） |
| `open` | 确保起着，然后打开浏览器 |
| `install` | 目录为空时，用上游 One-liner 装进去（`ANIMA_DIR` 指到 `-Dir`） |

参数：`-Dir`（引擎目录，默认 `<根>\training\anima_lora`）、`-WebUiDir`（WebUI 源码，默认自动在
`<根>\..` / `<根>\..\..` / `<根>\training\` 下找）、`-WebUiPort`（默认 8810）。

**实测（2026-10-18，引擎装在 `E:\anima_lora`，WebUI 在 `D:\myProject\anima-lora-webui`）**

```
pwsh -File scripts\train-engine.ps1 status   # 已安装 / 守护进程没起 / WebUI 没起
pwsh -File scripts\train-engine.ps1 start    # 守护进程就绪：端口 8765；WebUI 就绪：http://127.0.0.1:8810
                                             # WebUI 工作集 23.5 MB，可用内存 15.2 GB（起之前 15.3）
pwsh -File scripts\train-engine.ps1 stop     # 两个都停；8810 / 8765 都不再监听
```

## 3. 为什么是"外部引擎"而不是把 Python 嵌进来

训练栈 = Python + torch + 5GB 权重 + 数据集，**不可能**进 Rust 单文件 exe。所以形态跟 ComfyUI 一样：
ComfyHub 负责**目录 / 起停 / 端口 / 界面**，训练本身跑在引擎自己的进程里。

## 4. 接进 App（2026-10-21 已完成）

| 层 | 落点 | 说明 |
| --- | --- | --- |
| Rust 探测 / 起停 | `server-rs/src/train/mod.rs` | `discover_engine`（环境变量 → `<根>\training\anima_lora` → 同级/上上级 `anima-lora-next`）、`status`、`start`、`stop`；`pythonw` + `CREATE_NO_WINDOW`，日志落 `<根>\.run\train-*.log` |
| HTTP | `GET /api/train/status` · `POST /api/train/{start,stop}` | 只给界面"装没装 / 起没起 / WebUI 在哪个端口"；训练数据面仍在引擎 |
| AI 工具 | `tools/mcp.rs` 的第二条 MCP 桥 | 子进程 = 引擎自带的 `anima_daemon/mcp.py`；工具名统一前缀 `anima_`；读类 ALLOW、写类 ASK |
| Flutter | `lib/pages/train_page.dart` | 从 AI 工作台 AppBar 的「LoRA 训练台」进入；`webview_all` 内嵌 WebUI（2026-10-23 从 `flutter_inappwebview` 换过来，原因见 AGENTS §9.1）；URL host 取 `SettingsStore.baseUrl` 的 host，所以**手机连桌面地址时也能用同一页** |
| 生成配置 | 引擎 `gui.core.config_io` + `gui.core.submit.training_snapshot` | WebUI 调引擎自己的合并链，`path_scope = <slug>`；前后端都不手写第二份配置 |
| 参数面板 | 引擎 `anima_daemon/studio.py` 的 `/studio/projects/{slug}/config` + `anima_lora_webui` 的「参数」页 | **复现 Qt GUI 的 Config 页**：方法 / 硬件档、基础字段、可折叠高级分组；字段与中文帮助来自 `gui.core.config_io` + `_fields.json`，只写用户改过的 overrides；虚拟键（验证集 / 文件夹重复）复用 `gui.core.validation` 写进 `[[datasets]]` |
| Krea-2 面板 | `studio.py` 的 `/studio/projects/{slug}/krea2{,/train}` | 独立训练线的 `TrainRequest` 全表 + `stack_lora_*`（style-in-the-loop）；保存 `training_projects/<slug>/krea2.toml`，校验 cache/model 后以 `scripts/krea2/train.py --config` 提交到同一串行队列 |

**启动顺序**：Flutter 只调 `/api/train/start` → Rust 起 `anima_daemon`（等 `/health`）→
起 `anima_lora_webui`（挑空闲端口、等 `/api/_env`）→ 写 `<根>\.run\train-engine.json`。
WebUI 端口**只从这份 state 读**，和 `.run\api.json` 同一条纪律。

**手机 / 局域网**：Rust 起 WebUI 时绑 `0.0.0.0` 并生成一次性 token
（`--token`，写在 state 里）。`/api/train/status` 把 token 一起给 Flutter；
训练台打开 `http://<baseUrl 的 host>:<webuiPort>/?token=…`，WebUI 302 到 `/`
并种 HttpOnly Cookie，之后的 JS / 图片 / SSE 自动带。没有 token 的请求 401。
这样手机能连桌面，又不会把训练控制台裸露给整个局域网。

**两条独立 MCP 桥**（`comfy` 与 `anima`）：任何一条没装都只影响自己那批工具；
`warmup` 在后台各试一次，失败只记日志，不拦后端启动。

### 4.1 还没做（别当成已有）

1. **App 内的 `install`**：仍然只有 `train-engine.ps1 install`（上游 One-liner）。
   四档策略（已装好 → 借现成 → 建 venv → 才下载）照 `mcp-runtime.ps1` 的模子，但尚未实现。
2. **"复制 LoRA 到 ComfyUI 的 loras 目录"按钮**：产物路径已经由 `studio_status` 的
   `paths.output` 给出；复制动作还没做（手动复制可用）。
3. **项目删除**：WebUI 只提供筛选 / 重命名，没有"删除整个项目"的入口（避免误删数据集）。
4. **Qt GUI 的 Merge / TensorBoard / EasyControl 专属页**：WebUI 目前复现的是
   **参数表单**（Config）与整条数据 → 训练 → 作业链路；`easycontrol` 可以作为方法
   选到，但没有它单独的 dataset/preprocess 生命周期页。别把"参数页已对齐"当成
   "Qt GUI 的每个 tab 都搬过来了"。

## 5. 与仓库纪律的冲突点（实现前必须处理）

| 纪律 | 具体要求 |
| --- | --- |
| §2 无窗口 | 起引擎一律 `pythonw` + `Start-Process -WindowStyle Hidden`（脚本已这么做）；**不能**带 `--watch-stdin`（Start-Process 给的不是管道） |
| §12 受限 | 引擎**装在运行根目录里**时，它建的 `.venv` 会继承 Low 标签；建议装到工作区外（`-Dir D:\dist\anima_lora`），与发布包同口径 |
| §13 内容在自家文件夹 | 引擎自己会把 pidfile 镜像写到 `%USERPROFILE%\.anima\daemon.json`（**这是它唯一的散落点**）→ 起进程时给它 `ANIMA_DAEMON_PIDFILE=<根>\.run\anima-daemon.json`；HF 缓存用 `HF_HOME` 钉进引擎目录（上游 catalog 用 `local_dir` 直落 `models/`，本身不散） |
| §8 可选能力 | 缺引擎只是"不能在本机训 LoRA"；状态在界面上如实显示，**不许拦启动** |

## 6. 已验证 / 未验证

### 分辨率档怎么选（用户口径）

**8GB 显存是"够跑"的底线；决定档位的是宿主内存**：

| 显存 | 宿主内存 | 分辨率档 |
| --- | --- | --- |
| 8GB | **16GB** | **512 档**（如 448×576 ≈ 1008 tokens） |
| 8GB | **24GB** | **768 / 1024 档**（用户本机 4060 8GB / 23GB 实测 1024 可跑） |
| 12–16GB | 32GB+ | 1024 档起步 |

显存吃紧时的处置顺序：梯度检查点 → 块交换 → **降档** → 才动 rank。
这套口径写在 skill `skills/builtin/lora-training/SKILL.md`（AI 工作台会加载它）。

**已验证**（都是本机实测）：官方安装器按 `ANIMA_DIR` 装进任意目录（无需 git，5GB 依赖 9m37s）；
20 张图 20 步 LoRA 端到端（daemon 提交 → `done` rc=0 → 104MB 产物，8GB 卡峰值 7160MiB）；
`tasks.py preprocess` 全链路；Krea-2 内核在 512 档桶上 forward+backward（swap=20 + 梯度检查点，
显存 3.77GB / 宿主 7.5GB 可用）；WebUI 起停与内存；本脚本的 status/start/stop。

**2026-10-21 新增实测**：

* `studio` 全链路（隔离目录）：创建项目 → 导入 → 接受 → caption → 加触发词 → 生成配置，
  `training_snapshot` 的 `source_image_dir` / `resized_image_dir` / `lora_cache_dir` /
  `output_dir` / `datasets[*].subsets[*]` **全部** scope 到项目；拒绝会清掉 resized + latent。
* WebUI 同源反代：`/api/studio/projects` 经 8810 返回 200；静态页 / JS / CSS 正常。
* Rust `GET /api/train/status` 在真实引擎上返回 `installed=true`、`daemon/webui` 端口、
  `projects`；`/api/ai/tools` 里出现 27 个 `anima_*`（含 14 个 studio 工具）。
* Windows debug / release 构建（含 `webview_all_windows`）通过。旧实现 `flutter_inappwebview_windows`
  在插件注册时创建 Composition 合成器，关窗必弹 `Unknown Hard Error`，已整体换成 `webview_all 1.4.5`
  （2026-10-23；根因与关窗回归见 AGENTS §9.1）。
* Rust `train::start` 起 WebUI 时 `--host 0.0.0.0 --token <36 位>`：首跳 302 + HttpOnly
  Cookie，之后 `/api/_env` / 静态 / SSE 全部 200；无 token 的请求 401（实测
  `HttpClient` 带 CookieContainer 走通）。
* Android `flutter build apk --debug` 通过（JDK 25；`webview_all_android` 原生支持 AGP 9 内建
  Kotlin，不再需要补丁副本）。

**未验证**：Krea-2 的**完整 20 步训练**（缺 line 接线，见 `anima-lora-next/downstream/krea2-line-plan.md`）；
App 内的 `install`（仍只有脚本）；在手机上真机打开训练台（WebView 逻辑按 host 替换，
但本机只验了 Windows debug 构建）。
