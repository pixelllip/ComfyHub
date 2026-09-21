# 接入 comfy mcp：查证报告与结论（2026-09-21）

> 缘起：`docs/bug and suggestion/bug-and-suggestion-9.21.md` 的建议 ④
> ——「接入 comfy mcp，我们项目里和 mcp 重复的功能就可以删掉了」。

## ⚠ 最新决定（2026-09-21 晚，用户拍板，**以这一节为准**）

**用户决定：接官方 comfy-mcp，用它的能力；它没捆绑 comfy-cli，我们就用脚本在启动时检测并下载。**
（原话："但是官方mcp它覆盖的功能比我们更多更全，用它的。comfy没有自动捆绑 comfy mcp 下载，
但是我们可以做脚本，在启动时运行脚本检测下载。"）

下面第 1~5 节的"不整条接"，是当时**基于"要用户自己装 Python"这个前提**得出的；
用户给出了这个前提的解法（脚本自动装配），所以结论随之改变：

| 原来的顾虑 | 现在的处理 |
| --- | --- |
| 要 Python + comfy-cli，与发布包便携性冲突 | **`scripts\mcp-runtime.ps1` 把三样装进 `<根>\storage\ai\mcp`**（便携版 Python embeddable + pip 包），已实测跑通（见 §6） |
| comfy-cli 要一个"它认的工作区" | 装配的最后一步 `comfy set-default <用户真实在用的 ComfyUI>`，目录判据复用 `comfy-path.ps1`（与后端同一套） |
| 安全面更大（起进程 / 装节点 / 下模型） | 仍然**不把这些原样放给模型**：接进来之后按 §5 的"放行清单"逐项定权限档（默认 `deny` / `ask`），不是把 40 个工具全打开 |
| 我们的 `comfy_submit` 删不掉（产物要入库） | 这条**依然成立**：画廊入库 / NSFW 标记 / 审批闸门是我们自己的链路，MCP 只把文件写进 ComfyUI 的 `output/`。所以保留的是**入库与审批**，让出的是**工作流执行与节点/模型查询**这一类 |

**落地顺序**（每步一个提交）：
1. ✅ MCP 运行时装配脚本 + `up` / `doctor` 接线 + ComfyUI Desktop 嵌套探测（提交 `8f0ca60`）
   —— 体检分**快慢两档**（提交 `7e1b363`）：`up` 走文件检查（6ms），`doctor` / `-Deep` 才真问 comfy-cli（3.1s）
2. ✅ 后端 MCP stdio 客户端（提交 `21fae51`）：`initialize` → `tools/list` → `tools/call`、运行时发现、
   工具桥接与权限档，含 **6 例真起子进程的端到端测试**
3. ✅ 工具注入（提交 `2cb3a8c`）：**实测 22 + 39 = 61 个工具**（日志：`10 个默认 deny / 6 个默认 ask`），
   同名时我们的工具胜出（`comfy_submit` 那条"产物必须入库"的链路必须保住）
4. ⬜ 安卓客户端预留：见 [`android-client-plan.md`](android-client-plan.md)
   （桌面 App 当能力宿主、移动端只留不依赖本地工作流的工具；**入站鉴权是开远程的前置条件**）

### 现状（2026-09-21 实测）

| 项 | 状态 |
| --- | --- |
| 装配 | `pwsh -File scripts\mcp-runtime.ps1 status` → OK；comfy-mcp 0.10.0 / comfy-cli 1.20.0 / python 3.12.10 |
| 工作区 | `D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI`（ComfyUI Desktop 新版嵌套路径，探测已补） |
| 工具 | `GET /api/ai/tools` → 61 个；其中 39 个来自 comfy-mcp |
| 权限 | 10 个 `deny`（起停进程 / 装节点 / 下模型 / 云账号），6 个 `ask`（写盘 / 吃显卡 / 花钱），其余 `allow` |
| 降级 | 运行时不在 / 没 Python / 拉清单失败 → **一个 MCP 工具都不加，App 照常启动**（只记一行日志） |

下面第 1~5 节保留原始查证过程与数据（仍然有效，尤其是 §3 的逐条对照与 §4 的坑）。

---

> 这份文档是**结论 + 证据 + 可复现步骤**，不是实施方案。
>
> 报告当时的结论：**整条接入不划算，我们重复的功能删不掉；但官方 MCP 里有 5 项能力我们没有、值得用原生 HTTP 抄过来。**
> 下面每一条都给了出处，你可以自己核。

---

## 1. 查证方法与可复现步骤

| 做了什么 | 命令 / 出处 | 拿到的结果 |
| --- | --- | --- |
| 读官方文档 | <https://docs.comfy.org/agent-tools/mcp>（中文版同页） | 两套连接方式、云端工具表、本地工具表 |
| 把**本地服务端源码**拉下来读 | `git clone --depth 1 https://github.com/Comfy-Org/comfy-mcp` | HEAD = `e5f768d31de21ea32829381cebdda5336492a8ac`（2026-09-18，`ci: bump groom …`） |
| 版本与依赖 | 克隆里的 `pyproject.toml` | `name = "comfy-mcp"`、`version = "0.10.0"`、`description = "MCP server for ComfyUI — a thin wrapper over comfy-cli."`、`requires-python = ">=3.10"`、依赖 `mcp` SDK **2.x**（1.x 不行：2.0.0 把 `mcp.server.fastmcp` 直接删了，没有兼容层） |
| 逐个数它的工具 | `server.py` 里 `@mcp.tool()` 装饰器（共 **40** 个） | 见 §3 的表 |
| 看它是怎么干活的 | `server.py` 里的 `_run_comfy(...)`（起 `comfy` 子进程）、`target.py` | **每个工具都是 `comfy-cli` 的薄壳** |
| 本机现状 | `where.exe comfy` / `python --version` | `comfy`：**没有**；`python`：3.13.1（`C:\Users\Administrator\AppData\Local\Programs\Python\Python313\python.exe`，另有 miniconda） |
| 我们这边现状 | `grep -r "system_stats\|/free\|object_info" server/src/main/kotlin` | 只有 `/object_info`（`ComfySubmitter.objectInfo()`）；**没有** `/system_stats`、没有 `POST /free`、没有模型清单 |
| 现场探活 | `Invoke-RestMethod http://127.0.0.1:8188/system_stats` | 当时 ComfyUI **没在跑**（`目标计算机积极拒绝`），所以接口形状以文档与本机实测代码为准，见 §4 的两处订正 |

> ⚠ **每个工具名下面都跟着一个 MCP 提示词（tool description）**，比如 `run_workflow` 的说明里连"UI-export
> 太老时会检查 0 个节点却报 `valid: true`"这种盲区都写出来了。这些东西对我们的价值不只在代码，
> 也在那批"踩过的坑"的描述文本 —— 值得抄进我们自己的工具说明（§4.4）。

---

## 2. 官方 Comfy MCP 有哪两套

[官方文档](https://docs.comfy.org/agent-tools/mcp)给了两条连接，**不是二选一，可以并存**：

| | Comfy Cloud MCP | 本地 Comfy MCP（[`Comfy-Org/comfy-mcp`](https://github.com/Comfy-Org/comfy-mcp)） |
| --- | --- | --- |
| 地址 | `https://cloud.comfy.org/mcp`（远程 HTTP + OAuth / `X-API-Key`） | 没有 URL：客户端把 `comfy-mcp` 命令作为**子进程**拉起，走 **stdio** |
| 依赖 | Comfy 账号；**跑生成要有效订阅**（发现类工具免费）；`get_output` 只回临时签名 URL + 一条 `curl` 命令，要客户端自己在终端执行 | **Python ≥3.10 + `comfy-cli≥1.14.0`（在 PATH 或 `COMFY_BIN`）+ 一个 comfy-cli 管得着的工作区**；ComfyUI 必须**先自己起来**（服务端不会隐式启动） |
| 强项 | 预置模板库、`partner_generate`（Flux / Grok / Gemini / OpenAI / Ideogram / Seedream…）、批处理、云画布链接 | **读你本机真实安装**：自定义节点、盘上的模型、日志、显存；40 个工具 |
| 与本项目的关系 | 出网、要账号、要订阅，产物在云上 —— 与"本地创作工具、产物进自己画廊"的定位不符 | 与我们**功能重叠最多**，也是建议 ④ 说的那个 |

**本地这套的定位，官方自己的原话就是"a thin wrapper over comfy-cli"**（`pyproject.toml` 的 description）。
换句话说：**它没有自己的 ComfyUI 逻辑**，所有活都是 `comfy` 子进程干的。

---

## 3. 逐条对照：我们的 22 个工具 vs 官方 40 个

我们这边的真源是 `server/src/main/kotlin/com/comfyhub/ai/tools/ToolRegistry.kt` 的 `tools` 列表
（22 项，逐个数过：4 个 skill、1 个 memory、3 个 files、13 个 comfy、3 个 web/ask —— 其中 `web_search`/`web_fetch`/`civitai_search`/`ask_user_question` 与 Comfy 无关）。

### 3.1 真·重叠（MCP 那边有对应物，**但我们删不掉**）

| 我们的工具 | 官方对应 | 为什么不能直接换 |
| --- | --- | --- |
| `comfy_get_status` | `server_info` + `system_stats` | 我们的是 `ComfyCapture.status()`：连通性 + 队列 + **最近捕获记录 + 最近提交记录**，那是**我们的库**；MCP 给的是 comfy-cli 的环境与硬件 |
| `comfy_get_run` | `job(action="status"/"error")` | 我们按 `runKey` 查的是 `capture_runs`（含 import:`sha256`）；MCP 只认它自己 submit 过的 `prompt_id` |
| `comfy_sync_history` | `job(action="queue")` 等 | 我们是"把 `/history` 收进**我们自己的库**"，这条不能外包 |
| `comfy_load_workflow` | `run_workflow` / `validate_workflow` / `get_template` | 见 §3.4：我们做的是**界面格式 → API 格式**转换 + **##模拟用户行为##**，MCP 把这件事交给 comfy-cli，而 comfy-cli 自己承认这块有盲区 |
| `comfy_find_workflow` / `comfy_list_workflows` | `project` / `search_templates` | 我们列的是**用户自己的** `user\<用户>\workflows\*.json` 与**我们库里的**提示词；MCP 列的是 comfy-cli 的模板库与工作区 |
| `comfy_submit` | `run_workflow` / `run_template` | **这条最要紧**：我们的产物必须走 `capture.captureRun()` 入库（画廊、`mediaIds`、NSFW 标记），MCP 只把文件写进 ComfyUI 的 `output/` |
| `comfy_use_attachment` | `upload_file` | 见下 |

**`comfy_use_attachment` 那一行值得单独说**：MCP 的 `upload_file` 要的是"**这台机器上真实存在的绝对路径**"，
而它的工具说明里自己写着 ——「an MCP server never receives attachment *bytes*（协议没有客户端→服务端的附件通道），
所以要看客户端有没有把绝对路径注进上下文（Claude Code 会注入 `[Image: source: <path>]`），
没有的话就让用户自己存一份再把路径给过来」。我们的 `comfy_use_attachment(attachmentId)` 是**从我们的附件库直接投放**，
用户在聊天框里粘一张图就行 —— **这一条 MCP 明显不如我们**。

### 3.2 我们没有、值得抄（**都不需要 comfy-cli**）

这五项官方 MCP 有、我们没有，而且全都是 **ComfyUI 自己的 HTTP 接口**（不是 comfy-cli 的功能）：

| 官方工具 | 它到底做了什么（源码/文档核实） | 抄过来的价值 |
| --- | --- | --- |
| `system_stats()` | 透传 ComfyUI 的 **`GET /system_stats`**（每卡 `vram_free`/`vram_total`、`system.ram_free`/`ram_total`/`comfyui_version`） | 用户报"显存不足"时，模型现在**只能猜**。有了它就能先看余量、跑完再看一次确认 |
| `free_memory(unload_models=True)` | **`POST /free`**（`{"unload_models":…,"free_memory":…}`） | 大图/大模型切换前的常规操作，现在是空白 |
| `search_models(query, folder)` | `comfy models …`，**文件名**匹配，可跨目录（`models search --text`） | 模型该用哪个 LoRA/底模，现在是模型凭记忆猜。注意：**不需要 comfy-cli 也能做** —— 模型清单就藏在 `/object_info` 里（`CheckpointLoaderSimple.ckpt_name` 的枚举就是 checkpoint 列表，`LoraLoader.lora_name` 就是 LoRA 列表） |
| `validate_workflow(path)` | `comfy validate`：拿**活着的 `/object_info`** 预检 class_type / 输入形状 / 枚举 / 接线 | 我们只有 `comfy_check_inputs`（查"哪个输入还等着外部给"）与 `/object_info` 驱动的转换，**没有"提交前整体预检"**。官方自己在说明里列了盲区（缺必填输入、`COMFY_DYNAMICCOMBO_V3` 子输入、老 UI 导出会检查 0 个节点却报 `valid: true`）—— 这些坑正好告诉我们该往哪儿加判据 |
| `list_workflow_slots` / `set_workflow_slot` / `vary_workflow` | `comfy workflow slots/set/vary`：把前端格式工作流暴露成稳定地址 `6.text` / `115/75.strength`（子图内） | 我们的 `overrides` 键是**节点id.输入名**，格式一样；但"一份工作流暴露了哪些可调参数"现在要靠模型读图猜。`vary_workflow` 的"同一份图跑一组变体"对批量出图有用（用户有 anima 批量生成的习惯） |

### 3.3 我们**刻意不做**、MCP 做了的（不能抄）

| 官方工具 | 它做的事 | 我们的规矩 |
| --- | --- | --- |
| `install_node` | 装自定义节点（调 ComfyUI-Manager） | 改用户的 ComfyUI 安装 |
| `download_model(url, …)` | 下模型进 models 目录（多 GB，后台任务） | 写用户磁盘，且要出网下大文件 |
| `launch_comfyui` / `stop_comfyui` / `restart_comfyui` / `update_comfyui` / `switch_comfyui_version` | **起停/更新/换版本本地 ComfyUI 进程** | AGENTS §10：**没有 shell / 进程类工具**（AIH-045）。而且起停 ComfyUI 在我们这边是 `scripts\comfyhub.ps1` 的活（AGENTS §5：脚本是唯一入口） |
| `get_logs` / `discover` / `which` / `project` | 读日志、探 comfy-cli 环境 | 前两个要读用户机器上任意日志路径 |

### 3.4 关于"删掉重复功能"：唯一真能删的那块，恰恰不能删

我们真正"重复"的是 `comfy_load_workflow` 里那套**界面格式（nodes/links）→ API 节点图**的转换
（`WorkflowConvert.kt` / `FrontendNodes.kt`，含子图/Reroute/Set-Get/旁路展开与 **##模拟用户行为##**）。
把它换成 MCP / comfy-cli 的转换，会踩官方自己写在 `validate_workflow` 说明里的那条：

> 「a UI-export file too old to auto-convert checks **ZERO** nodes, reporting `valid: true`」

也就是说 **comfy-cli 的 UI→API 转换会把"什么都没检查"报成"通过"**。我们这套转换是逐节点判据、
说不清就如实报 `UNSUPPORTED_NODES`（`FrontendNodesTest` 5 例盯着"不开开关时行为与过去逐字一致"）。
**用会静默放过的东西，换掉一个会如实报错的实现，是退步。**

---

## 4. 三处需要订正/注意的细节（别按传闻写代码）

### 4.1 「起个 Python 子进程就能用」不成立

`comfy-mcp` 是 **MCP stdio 服务端**，它的每个工具都 `_run_comfy(...)` 去起 `comfy` 子进程；
而 `comfy-cli` **不随 `comfy-mcp` 安装**（README 原话：Installing this server does not install comfy-cli）。
所以要落地就必须凑齐：Python 运行时 + `comfy-mcp` 包 + `comfy-cli` 包 + 一个 comfy-cli 认识的工作区。
它的工作区可以用 `comfy set-default <path>` 指向**已有的 ComfyUI 检出**（README §Prerequisites 承认这一点），
所以"用户已经装了 ComfyUI"这件事本身不是障碍 —— **障碍是我们得替用户把这套 Python 依赖装上**。

### 4.2 远程模式（`COMFYUI_URL`）比想象中窄

`target.py` 里写得很死：只能 `http://host:port`，**不许带 path、query、fragment、`;params`，也不许 `https://`**
（理由：comfy-cli 只转发 `--host`/`--port`，带了会被静默丢掉）。而且 `system_stats` / `free_memory` /
生命周期工具**根本不接受 `--host`**：配了远程之后这两个工具读的仍是**本机**，服务端只能往 payload 里塞一个
`comfy_target_note` 提醒"这数字未必是关于你提交的那台机器"。我们只有一个本机 `comfyUrl` 配置，反而没这个歧义。

### 4.3 版本漂移风险

- `comfy-mcp` 要 **MCP SDK 2.x**（1.x 直接 import 失败）；
- `comfy-cli` 有硬下限 **1.14.0**，而且 `search_models` 的"词序无关 + 忽略分隔符"要 **>1.15.0**，
  在 1.15.0 上整串查询词被当**一个子串**；
- 官方自己标着 **Public Beta**：API、工具、行为都可能变。
对"发布包要能长期便携运行"的项目来说，这是把我们的稳定性绑在两条外部版本线上。

> 另外一条**必须说清的事实**：我们**当前**没有任何 MCP 客户端实现。
> 要真接，得先在 Kotlin 里写一个 stdio JSON-RPC 客户端（大致 4 步：起子进程 → `initialize` 握手 →
> `tools/list` → `tools/call`），再把工具动态注进 `ToolRegistry` 的 `tools` 列表。
> 注意 `ToolRegistry.tools` 的注解明说它是**唯一真源**，而 `comfyObjectInfo` / `comfySubmit` 这些
> 都是构造期注入的 lambda —— 动态加工具要动这个结构，不是加一个文件就完事。
> 规模：约 1000~1500 行 Kotlin + 一套"Python 缺失时整个 MCP 类别不可用"的降级设计。
>
> **一个更省事的中间选项：跳过 MCP 协议层，后端直接调 `comfy-cli`。** 反正 `comfy-mcp` 本身就是 comfy-cli 的薄壳，
> 那层 MCP 协议对我们没有额外信息。省掉客户端实现，但 **Python + comfy-cli 的依赖问题一点没省**。

---

## 5. 结论

1. **不建议整条接入**（不建议把我们的 comfy 工具换成 MCP）：依赖不可接受（Python + comfy-cli，与发布包可移植性冲突）、
   安全面更大（起停进程 / 装节点 / 下模型，违反 AIH-045 与"脚本是唯一入口"）、
   而且**真能删的只有那一套会被静默放过的 UI→API 转换** —— 换过去是退步。
2. **建议做**：把 §3.2 的 5 项能力**用 ComfyUI 自己的 HTTP 接口原生实现**（`GET /system_stats`、`POST /free`、
   `/object_info` 里的模型枚举、提交前预检、工作流槽位）。**不引入任何新依赖**，
   照现有 `ComfySubmitter` / `ComfyCapture` 的 `HttpClient`（`OutboundProxy.selector()` 已在用）路子写即可。
3. **要保持跟踪**：官方 MCP 是 Comfy 明确的方向（云端 + 本地+mcp），
   那批 tool description 里的"盲区清单"是可以直接抄进我们工具说明的现成踩坑记录（§4.4）。
4. 想**只是给自己用**（DSH 里跑 ComfyUI），那是配置的事，与本项目代码无关 —— 不需要改这个仓库一行。

> ⚠ 上面这 4 条是**报告当时**的结论。用户在 2026-09-21 晚拍板改走"接入"，见卷首那一节。

---

## 6. 装配实测记录（2026-09-21，本机真跑，不是推断）

落地方案见 `scripts\mcp-runtime.ps1`（文件头有完整说明）。这里只记**实测到的硬事实**，
因为其中好几条官方文档没写、踩了才知道：

| 事实 | 细节 |
| --- | --- |
| embeddable Python 能用 | `python-3.12.10-embed-amd64.zip`（10.6MB）解压后，要把 `python312._pth` 里的 `#import site` 打开、再补一行 `Lib\site-packages`，然后用官方 `get-pip.py` 装 pip —— 全程免安装、免管理员 |
| **embeddable 版没有 venv 模块** | 所以不能建 venv，是"整包一个环境"。对我们反而好：跑哪个解释器是可预测的 |
| 包能一起装上 | `comfy-cli` **1.20.0** + `comfy-mcp` **0.10.0** + `mcp` SDK **2.2.0**（comfy-mcp 需要 SDK **2.x**，1.x 直接 import 失败）。装完约 **300MB** |
| **清华 TUNA 源装不上 comfy-mcp** | 元数据能读，**wheel 返回 HTTP 403**。阿里云可用，官方 pypi.org 可用。所以源顺序是 阿里云 → 清华 → pypi.org |
| comfy-cli 必须设工作区 | 状态在 `%LOCALAPPDATA%\comfy-cli\config.ini`。**不设的话它默认报 `%USERPROFILE%\Documents\comfy\ComfyUI`**（目录不存在也照样这么报），于是 MCP 那边 `run_workflow` / `search_models` / `nodes` 全是错的 |
| ComfyUI Desktop 新版的嵌套路径 | 本机真实安装是 `D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI`（有 `main.py` + `comfy/`）。**只在顶层翻一层永远找不到** —— `comfy-path.ps1` 与 `ComfyLocator.kt` **两边都补了这条判据**（AGENTS §10.1 要求一致） |
| MCP 握手已验证 | spawn `comfy-mcp.exe` → `initialize`（协议版本 `2024-11-05`）→ `notifications/initialized` → `tools/list`，**实测返回 39 个工具**（带完整 `inputSchema`，无 `outputSchema`）。协议形状可以照这个写 Kotlin 客户端 |
| comfy-mcp 启动慢 | `initialize` 到 `tools/list` 之间它要跑 `comfy --version` 等探测，首次约几秒；客户端要有超时与"进程死了要重启"的处理 |

```powershell
# 复现整套装配（联网约 300MB）
pwsh -File scripts\mcp-runtime.ps1 install
pwsh -File scripts\mcp-runtime.ps1 status     # 期望：OK + python/comfy-mcp/comfy-cli 版本 + 工作区
```

---

## 附：本次查证的原始命令

```powershell
# 1) 拉官方本地 MCP 源码（本机 github 直连不通，走代理；git 已配 http.proxy=127.0.0.1:7890）
git clone --depth 1 https://github.com/Comfy-Org/comfy-mcp $env:TEMP\comfy-mcp-inspect
git -C $env:TEMP\comfy-mcp-inspect log -1 --format='%H %ad %s' --date=short
Get-Content $env:TEMP\comfy-mcp-inspect\pyproject.toml   # version 0.10.0 / requires-python >=3.10

# 2) 数它的工具（40 个）
Select-String -Path $env:TEMP\comfy-mcp-inspect\src\comfy_mcp\server.py -Pattern '^@mcp\.tool'

# 3) 本机环境
where.exe comfy      # → 空（没有 comfy-cli）
python --version     # → Python 3.13.1

# 4) 我们这边有没有用过这几个接口
Select-String -Path server\src\main\kotlin -Include *.kt -Recurse -Pattern 'system_stats|/free'
```
