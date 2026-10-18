# 训练引擎（可选能力）—— 现状、脚本、以及"接进 App"的落点

> 状态：**脚本层已落地并实测**（2026-10-18）。App 内的界面/Rust 桥还是待办（§4）。
> 一句话：训练引擎是**可选能力**，按 AGENTS §8 的纪律（检测优先、安装兜底、装不上不拦启动）走。

## 1. 引擎是什么

| 仓库 | 内容 | 备注 |
| --- | --- | --- |
| `pixelllip/anima-lora-next` | 上游 `sorryhyun/anima_lora` 的**下游整合版**：训练/推理/守护进程内核 + 中文界面补丁 + Krea-2 内核 | 以上游为基线，带"每日跟版"CI（[DOWNSTREAM.md](https://github.com/pixelllip/anima-lora-next/blob/main/DOWNSTREAM.md)） |
| `pixelllip/anima-lora-webui` | 自有 Web 界面（标准库，零依赖，常驻 ~25MB） | 替代上游那个有 bug、常驻 164MB 的 PySide6 GUI |

引擎的对外面只有一个：**`anima_daemon` 的 localhost REST**（JSON，无 CORS 头）：

```
GET  /health                    守护进程健康 + pid/port/root
POST /jobs                      {method,preset,methods_subdir,overrides,extra,start} 或 {kind:"command",argv,label}
GET  /jobs                      作业列表（含 state/rc/overrides/result_summary）
GET  /jobs/{id}                 单个作业 + latest/stale_for
GET  /jobs/{id}/progress?last_n=2000   结构化事件（ev=step|ckpt|log|run_start|run_end）
GET  /jobs/{id}/logs            stdout+stderr 的 SSE
POST /jobs/{id}/{stop,pause,resume} · /queue/{start,pause} · /shutdown
GET  /tools                    自描述清单（13 个操作，带 JSON-Schema）
```

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

## 4. 接进 App 的落点（待办）

1. **Rust：新域 `server-rs/src/train/`**（`mod.rs` + `engine.rs` + `routes`），照 `comfy::comfy_launch`
   的模子：探测引擎目录、读 pidfile、起停（`CREATE_NO_WINDOW`）、把上面那 6 个端点薄薄地转出去
   （**别在 Rust 里重写训练逻辑**）。设置项进 `app_settings`（`train.engineDir` / `train.webuiPort`）。
2. **Flutter：次级页**「训练」（AGENTS §3：一级导航只有 5 项，新功能挂次级页或设置页）。
   内容 = 引擎状态卡（目录 / 守护进程 / WebUI）+ 作业列表 + 提交表单 + 进度（loss 曲线与事件表）。
   原型已经在 WebUI 里验证过（同一套 REST），Flutter 只是换一层壳。
3. **自动下载**：`install` 那条 One-liner 的 App 版 —— 沿用上游 `ANIMA_DIR`/`ANIMA_VERSION`，
   指向**我们自己仓库的 release**；四档策略照 `mcp-runtime.ps1`（已装好 → 借现成 → 建 venv → 才下载），
   认 `COMFYHUB_NO_DOWNLOAD`，失败不拦启动。
4. **产物入口**：训练产出的 LoRA 落在 `<引擎>\output\ckpt\`，元数据里带 `ss_adaln_layout=comfy`
   （**可直接被 ComfyUI 的 LoRA loader 读**）——App 里给一个"复制到 ComfyUI 的 loras 目录"的按钮，
   而不是自己写第二套加载器。

## 5. 与仓库纪律的冲突点（实现前必须处理）

| 纪律 | 具体要求 |
| --- | --- |
| §2 无窗口 | 起引擎一律 `pythonw` + `Start-Process -WindowStyle Hidden`（脚本已这么做）；**不能**带 `--watch-stdin`（Start-Process 给的不是管道） |
| §12 受限 | 引擎**装在运行根目录里**时，它建的 `.venv` 会继承 Low 标签；建议装到工作区外（`-Dir D:\dist\anima_lora`），与发布包同口径 |
| §13 内容在自家文件夹 | 引擎自己会把 pidfile 镜像写到 `%USERPROFILE%\.anima\daemon.json`（**这是它唯一的散落点**）→ 起进程时给它 `ANIMA_DAEMON_PIDFILE=<根>\.run\anima-daemon.json`；HF 缓存用 `HF_HOME` 钉进引擎目录（上游 catalog 用 `local_dir` 直落 `models/`，本身不散） |
| §8 可选能力 | 缺引擎只是"不能在本机训 LoRA"；状态在界面上如实显示，**不许拦启动** |

## 6. 已验证 / 未验证

**已验证**（都是本机实测）：官方安装器按 `ANIMA_DIR` 装进任意目录（无需 git，5GB 依赖 9m37s）；
20 张图 20 步 LoRA 端到端（daemon 提交 → `done` rc=0 → 104MB 产物，8GB 卡峰值 7160MiB）；
`tasks.py preprocess` 全链路；Krea-2 内核在 512 档桶上 forward+backward（swap=20 + 梯度检查点，
显存 3.77GB / 宿主 7.5GB 可用）；WebUI 起停与内存；本脚本的 status/start/stop。

**未验证**：Krea-2 的**完整 20 步训练**（缺 line 接线，见 `anima-lora-next/downstream/krea2-line-plan.md`）；
App 内的 Rust/Flutter 部分（还没写）；`install` 动作用我们的 release（目前指向上游安装器）。
