//! ComfyHub 桌面服务端（**Rust 版**，P4 灰度中）。
//!
//! ```
//! comfyhub-server-rs --db D:\.db\comfyhub.db --port 8081
//! ```
//!
//! # 谁起我、谁关我（2026-09-30 起）
//!
//! **桌面 App 自己直接拉这个 exe**，不再经过 `scripts\comfyhub.ps1` / `server.ps1` /
//! WMI（那套脚本已按用户要求删掉）。契约在 `lib\core\backend_launcher.dart`，参数是：
//!
//! ```text
//! <根>\backend\comfyhub-server-rs.exe --db <根>\.db\comfyhub.db --storage <根>\storage \
//!     --host 127.0.0.1 --port 0 --watch-stdin
//! COMFYHUB_PROJECT_ROOT=<根>      （环境变量，端口文件与工具权限的基准）
//! COMFYHUB_ALLOW_REMOTE=1         （开了「允许手机 / 局域网设备连接」时才有）
//! ```
//!
//! 三条纪律（改动启动方式前先读）：
//!
//!  1. **端口永远是 `0`**（让操作系统挑），真实端口由我们写进 `<根>\.run\api.json`，
//!     App 只读它 —— 所以 App 不能假定一个固定端口（`api_port_file.rs`）；
//!  2. **`--watch-stdin` 是"App 强杀后不留孤儿"的唯一保障**（见 [`spawn_stdin_watchdog`]）；
//!  3. **`COMFYHUB_PROJECT_ROOT` 由 App 显式给**：我们的兜底是"从 storage 往上推"，
//!     而发布包里 `backend\` 与根目录差一层，纯靠推断容易把 `.run\` / `.db\` 落到别处。
//!
//! # 灰度期怎么跑
//!
//! Kotlin 版仍在 `8080` 上服务（它现在是唯一的写者），Rust 版**默认 8081**，
//! 只提供已搬过去的只读端点。两端点并存 → 逐个端点搬 → 全部稳定后再切脚本与发布包
//! （总纲 §5.1 的"按端点灰度"）。
//!
//! # 与 Kotlin 版共享同一个库文件
//!
//! Rust 版打开的是**同一个 `comfyhub.db`**（WAL）。三条纪律照抄：
//! ① 每条连接都上 PRAGMA（[`db::open`]，红线 R1）；② 写事务一律 `BEGIN IMMEDIATE`
//! （[`db::tx_immediate`]，R2）；③ **不建表**（schema 的真源是 `db/schema.sqlite.sql`，
//! 迁移仍然归 Kotlin 侧 —— 等 P4 收口时再把这个职责搬过来）。
//!
//! # 现在有什么、没有什么（诚实清单）
//!
//! | | 状态 |
//! | --- | --- |
//! | `GET /api/health`（含 SQLite 版本 / FTS5 自检 / schemaVersion） | ✅ |
//! | `GET /api/tags`、`/api/tags/categories` | ✅ |
//! | `GET /api/prompts`（搜索 / 分页 / 排序）、`/api/prompts/{id}` | ✅ |
//! | `/api/media`（列表 / 详情 / **原件文件流** / 缩略图 / 上传 / PATCH / DELETE） | ✅ |
//! | `GET /api/stats`、`/api/capture/config` | ✅ |
//! | `GET /api/capture/workflows`、`/api/capture/locate{,/apply}` | ✅ |
//! | **`GET /api/capture/status` / `/jobs`**（真的去问一次 ComfyUI 的 `/queue`） | ✅（P4 第 42 步） |
//! | **`POST /api/capture/poll`**（拉一次 `/history` 并**入库**：建提示词 + 产物进画廊） | ✅（P4 第 43 步） |
//! | **`POST /api/ingest/comfyui`**（推送式捕获：外部把一条已跑完的运行推过来，幂等） | ✅（P4 第 49 步） |
//! | **`POST /api/capture/import-workflows`**（把本机 ComfyUI 里已保存的工作流文件批量读进库；界面上的「导入本机工作流」按钮） | ✅（P4 第 49 步：`run_key = file:<sha256>` 判重，导两次只有一行） |
//! | **后台捕获轮询器**（用户自己在 ComfyUI 里跑的东西也会被收）+ `/api/capture/status` 的 `lastPollAt` / `lastError` | ✅（P4 第 47 步：`capture_poller.rs`，逐条对齐 Kotlin `ComfyCapture.start()`；启动等 2.5 秒、停用 5 秒空转、间隔夹 1..600、同一时刻只跑一轮） |
//! | `GET /api/{prompts,media}/{id}/workflow`、`GET /api/prompts/{id}/api-graph` | ✅ |
//! | **`/api/sync/{manifest,changes,push,bootstrap}`**（4 条全落地） | ✅ **含 `kind:"submit_job"` 那条指令**（M5，契约 §4.1）：手机排队 → 桌面**先过审批闸门**（"手机点过提交"不算批准）→ 批准后走**现有 `comfy_submit` 全链路**（R10）→ 结果作为一条助手消息追加进发起那条会话（手机靠普通下行同步看到）。落点 `submit_queue.rs` + `submit_jobs` 表；`push` 本身**不阻塞**（受理 = 落一行 + 起后台任务） |
//! | `GET /api/ai/providers`（不含密钥）、`GET /api/ai/providers/{id}/models` | ✅ |
//! | **Provider 写侧**：`POST/PUT/DELETE /api/ai/providers`（乐观锁 AIH-007）、`PUT …/models` | ✅（P4 第 37 步） |
//! | **Provider 连接测试 / 模型发现**：`POST /api/ai/providers/{id}/{test,discover-models}` | ✅（P4 第 53 步：`provider_probe.rs`；`/models` 的两种 Base URL 写法都试、404 不算鉴权失败、密钥脱敏、能力**逐维度**合并内置目录） |
//! | **凭据写侧**：`GET/PUT/DELETE /api/ai/providers/{id}/credentials`、`/api/ai/web-search/credential` | ✅（P4 第 34 步，DPAPI 与 Kotlin 共用一份文件） |
//! | **Skills 六条**：`/api/ai/skills{,/roots,/rescan,/{name}}` | ✅（P4 第 35 步） |
//! | **长期记忆五条**：`GET/PUT/DELETE /api/ai/memory`、`POST …/entries`、`POST …/delete` | ✅（P4 第 36 步） |
//! | **内置模型目录**：`GET /api/ai/builtin/status`、`POST /api/ai/builtin/sync` | ✅（P4 第 38 步） |
//! | **能力目录状态与实测校准**：`GET /api/ai/capabilities/status`、`PUT …/probe-toggle`、`POST …/refresh`、`POST …/probe` | ✅（P4 第 39 步；`refresh` 与 `probe` 见上表那一行） |
//! | `GET /api/ai/conversations`、`/{id}`、`/{id}/messages`（**读侧**：历史会话与有序块） | ✅ |
//! | **角色卡四条**：`/api/ai/character-cards{,/{id}}`（列表 / 新建 201 / 详情 / 全量编辑 / 删除） | ✅（P4 第 46 步：`character_cards.rs`；2026-10 收敛成**名称 / 正面提示词 / 负面提示词**，`/summary` 那条一起删了） |
//! | **资料库两件工具**：`search_character_cards` / `save_character_card`（写角色前先查本机） | ✅（P4 第 46 步；2026-10 随角色卡一起只剩名称 / 正面提示词 / 负面提示词） |
//! | `POST/PATCH/DELETE /api/ai/conversations`、`POST …/messages`（**写侧**） | ✅ |
//! | **Run 生命周期**：`POST …/runs`（202）/ `GET /runs/{id}` / `GET /runs/{id}/events`（**SSE**）/ `POST /runs/{id}/cancel` | ✅（事件流按 `?after=` 从库里拉，见 `run_lifecycle.rs`） |
//! | **`GET /api/ai/run-kit{,/status}`**（手机 harness 的全部规则类输入；R12/R13 的第一次重建） | ✅ |
//! | **`/api/ai/pending`**（跨端待办总览：还在等人的提问、工具审批，**以及手机排队待批的提交**） | ✅ |
//! | **`/api/auth*` 配对闸门 + `GET /api/ai/credentials` 密钥下行 + `PUT` 密钥上行**（inbound auth P0 + 配对第 2 步 / §14） | ✅ 下行的开关是 `allowCredentials`、上行是**另一个**开关 `allowCredentialsUpload`（两个都默认关，§14.1）；上行按 `ai.credentials.rev` 做"等值才应用"（落后回 `STALE_IGNORED`），逐条独立（env 只读 → `FORBIDDEN`、坏名字/空值 → `BAD_PAYLOAD`） |
//! | 扫码配对（`POST /api/auth/pair/start`） | ✅（2026-09-27：**仅回环**。回 `ticket` + 6 位码 + `urls[]`（来自 `net_urls`）+ 两个过期时刻；二维码那串 URI 由**前端**编（`lib/core/pairing_payload.dart` 是格式的唯一实现）。扫码会话与"手机举手"共用同一套 `Session`，所以 `pair/claim` 两条路都认；`ticket` 只在这个响应里出现，不进 `pair/pending` —— 那张列表是给手敲那条路用的） |
//! | `POST /api/ai/capabilities/{refresh,probe}` | 两条**都是实现**。`refresh`（2026-09-27）：从 `https://models.dev/api.json` 拉在线数据集 → 规范化成覆盖层写 `<storage>\ai\capabilities\remote.json`（不重启、不发版）；失败**不抛错、旧目录原样保留**，结果用 `lastMessage` 如实回给界面；`COMFYHUB_NO_DOWNLOAD=1` 时如实说"已关闭在线更新"。`probe`（2026-09-27，`capability_probe.rs`）：对每个推理模型**从最高档往下**试极小请求（`content:"ping"` + `max_tokens:256` + 每请求 30 秒上限），把网关**实际接受**的档位写回 `custom.json`（只动这一个模型的 `thinkingEfforts`，别的条目一个字节都不变；读坏 / 坏 JSON 一律如实报错、绝不覆盖）；**默认关**（`PUT …/probe-toggle`，关着一个上游请求都不发），一个档位都没认就如实 `ok:false` + 不发明档位、不写回 |
//! | **上游 LLM 客户端 / 协议适配器**（第 29 步）与**主循环**（第 30 步） | ✅ 已落地（不带工具、不带附件）；工具层已搬四个本机工具 + 两个闸门（第 31/32 步） |
//! | **桌面文件工具五件**（`list_dir` / `read_file` / `write_file` / `register_skill` / `delete_skill`）+ **路径准入**（`resolve_read` / `resolve_write`：禁段、真实路径、目录根不可写） | ✅（P4 第 40 步；`desktop_tools.rs` + `policy.rs`，桌面提示词与下发的 `tools[]` 现在含它们） |
//! | **comfy 五件**：`comfy_get_run` / `comfy_list_workflows` / `comfy_get_status` / `comfy_sync_history`（写库：建提示词 + 产物入库）/ `comfy_load_workflow`（把本机工作流文件读进库，界面格式按 `/object_info` 转换） | ✅（P4 第 41~44 步） |
//! | **`comfy_submit`（R10）**：审核 → 审批 → 提交 → **自己等** → `captureRun` 入库 → `mediaIds` | ✅（P4 第 48 步：`workflow_submit.rs`；参数覆盖 / 补线逐条移植，超时如实报 `timeout`；`/api/capture/jobs` 的 `submissions[]` 也由它维护。**第 70 步把闸门接全**：`workflow_audit::blocking_refusal` 那三项"必跑不了"（前端节点 / 版本不兼容 / 本机没有的节点类）当场拒（`MODEL_NOT_LOADABLE`，与 Kotlin 同码同措辞），参数 / 显存 / 提示词风格那些非阻断的走 `preflightWarnings` **只提醒不拦**） |
//! | **生成任务的实时进度 + 当前步骤**（`comfy_progress.rs`：常驻 `ws://<comfyUrl>/ws` + 指数退避重连；`/api/capture/{status,jobs}` 的 `current` 与 `submissions[].progress`、`comfy_get_status` 的 `current` / `currentStep` / `progressConnected`） | ✅（P4 第 72 步。REST 拿不到进度 —— 2026-09-27 实测 `/queue` 没有、`/progress` 404、`/history` 只有跑完的，所以这是唯一来源。`percent` **只**来自 ComfyUI 自己发的 `value/max`（`progress` / `progress_state` 帧），**拿不到就是 `percent` 缺席（进度未知）**，绝不用"已执行节点数/总节点数"冒充；终态也不把 percent 抬到 100） |
//! | **`GET /api/net/urls`**（这台桌面有哪些**可达地址**：回环 / 局域网 / Tailscale） | ✅（P4 第 73 步：`net_urls.rs`。给手机的「同步与连接」页当候选 —— 手机没法自己扫网段猜桌面地址，而"能不能连上"由客户端打一次真实请求决定，分类只用于显示。端口用**我们实际监听的**那个；`port == 0` 时如实回空数组，不编地址） |
//! | **联网三工具 + 定向查询**（`web_search` / `web_fetch` / `civitai_search` / `find_model`） | ✅（P4 第 64 步：`web_tools.rs` / `civitai.rs` / `find_model.rs`。四件都是 **WEB 档**，共用 `MAX_WEB_QUERIES_PER_RUN` 那份预算、出厂全 `ASK`；`web_fetch` 的目标过 SSRF 闸门，`find_model` **先查本机再查外网**且**绝不代下**） |
//! | **官方 comfy-mcp 那 39 个工具**（`system_stats` / `nodes` / `run_workflow` / `launch_comfyui` / `get_logs` …） | ✅（P4 第 45 步：`mcp.rs` 起 `comfy-mcp` 子进程走 JSON-RPC，清单进 `/api/ai/tools` 与下发的 `tools[]`；`launch_comfyui` 由我们**替用户补上共享模型根那三个参数**，失败时把 stderr 尾部带进 `detail`）。⚠ **起 / 停实例请走 `comfy_start` / `comfy_stop`**（2026-09-28）：comfy-cli 启动时要写两个**项目外**的文件（`%LOCALAPPDATA%\comfy-cli\config.ini` 与 `<ComfyUI>\user\comfyui_8188.log`），受限会话里必然 `Errno 13`，而第二个没有参数可改 —— `launch_comfyui` 那条路的失败提示现在直接指过来 |
//! | **自己起 / 停 ComfyUI**：`comfy_start` / `comfy_stop` | ✅（2026-09-28，用户 bug「通过 CLI 启动 comfyui 总是失败」：`comfy_launch.rs` —— 用工作区自己的 `.venv\Scripts\python.exe` 跑 `main.py`（三个路径参数复用 `mcp::prepare_launch_with`）、`PYTHONUTF8=1`、日志落项目内 `.run\`、pid 只认自己起的那个；起之前先探写权限，**写不进就当场如实拒绝**——子进程**继承**当前进程的限制，实测 WMI / schtasks 全被系统挡掉 |
//! | **关键步骤（环节）折叠**：`[步骤]…[/步骤]` → `step` 有序块 + `step.started` 事件 | ✅（2026-09-28，用户建议：`step_mark.rs` 流式摘取六个固定环节名 + 系统提示**第 21 条**（`PROMPT_VERSION` v35）+ 界面按环节把连续的工具卡折进一组；摘取**绝不吞内容**，枚举外的名字摘掉但不记环节） |
//! | **工作流审核 + 摘拼**：`comfy_audit_workflow`（七项确定性检查）/ `comfy_prune_workflow`（可达性摘除 + 按 `/object_info` 拼装） | ✅（P4 第 63 步：`workflow_audit.rs` / `workflow_prune.rs`；两件都**只报不改**，改好的图是**内联**返回的） |
//! | **工作流配方三件**：`comfy_learn_workflow` / `comfy_list_recipes` / `comfy_bind_recipe` + `workflow_recipes` 表 + `/api/ai/workflow-recipes*` | ✅（P4 第 67 步：`workflow_recipe.rs` —— 归纳成「任务 + 骨架指纹 + 模型槽位 + 参数旋钮」，默认**拒学**转换有缺口的图（`UNSUPPORTED_NODES`），绑**只报不改**；学习与绑定与端点**共用一份实现**。**系统提示第 20 条**（P4 第 69 步）已按 Kotlin 原文补进常驻段，`PROMPT_VERSION` 随之 **v32 → v33**；正文真源是 `src/nsfw_prompt.rs`（`gen_prompts.py` 已停用）） |
//! | **2D 布局模板**：`layout_propose`（`ai/layout_proposal.rs`）+ 工作台里的模板卡 / 「提交回工作台」 | ✅（2026-11-03 用户换思路："AI 生成提示词的同时给出可用的 2D 布局模板，我可打开预览并修改，修改后重新提交给工作台"。工具只出构图草稿（0~1000 框 + 关系），**不生成图片 / 不写库**；后端夹边、补 id、清坏关系并逐条记账，`layoutTemplates` 落进 `tool_result` 块，重开 App 卡片还在。**3D 路线同日搁置**：`docs/layout-3d-design.md` 只留作评估。契约见 `docs/layout-workspace-design.md`） |
//! | **`WorkflowConvert` 的两处欠账** | ✅ **清零（P4 第 68 步）**：**组节点（subgraph）按定义展开**成内部真节点（`flatten`：重新编号 / 外层控件值填进内部输入 / 输出代理登记成改写）；**「模拟用户行为」（`emulateFrontend`）真的实现**（`workflow_frontend.rs` = Kotlin `FrontendNodes.kt` 的对照：广播填空 / 丢显示节点 / 补 `SaveImage`），改了什么都如实列在 `frontendEmulated` / `frontendBroadcast` / `frontendInjected` 里。工具描述是 Kotlin 的逐字拷贝，**现在它说的是真的**，"本后端还没做这一步"那句 warning 已删除 |
//! | 附件（R9）：`/api/ai/attachments*`、`/api/ai/preflight` 与内联预算 | ✅ **已搬（P4 第 50 步）**：五条端点（上传 multipart 字段 `files` / 详情 / 原件 ETag+Range / 缩略图 / 删除）+ **严格签名判定**（`file_kind.rs`）+ 三层准入（`strict_intake.rs`，第三层在 `POST /runs` 之前、**上游零请求**）+ **图片真的内联进请求体**（三家适配器各自的形状）。⚠ 仍缺：**视频预览帧**（`/thumb` 回 204）与视频/音频/文档的传输方式（三家都只有图片的 `inline_base64`） |
//! | `/api/capture/{import,import-workflows}`、`/api/ingest/comfyui` | ✅ **三条全搬（P4 第 49/52 步）**：`import-workflows`（批量读工作流文件）、`ingest/comfyui`（推送式捕获）、`import`（导入目录里已生成好的产物，**PNG 内嵌的 prompt 会还原成提示词**） |
//! | **`/api/media/{id}/poster`**（视频第一帧封面） | ✅（P4 第 51 步：`poster.rs` 起 `scripts\video-poster.ps1`（Windows 缩略图管线，**不依赖 ffmpeg**）；抽不出帧如实回 204） |
//! | R6~R10 的安全防线 | ⛔ **部分重建**（R5 的保留窗口与 seq 纪律已在 `runs.rs`/`events.rs`，R6/R8 已在执行器与工具层，R9/R10 未动） |
//!
//! ## 手机现在可以只对着**这一个后端**干活了（2026-09-24，第 22 步之后）
//!
//! 在此之前手机上那两条路各需要一台不同的后端：`GET /api/ai/run-kit` 只在 Kotlin，
//! `POST /api/sync/push` 只在 Rust —— 手机"发得出去"与"推得回去"没法同时成立。
//! `/api/ai/run-kit` 搬过来之后，**Rust 侧同时有这两条**，真机回归才有意义。

use comfyhub_server_rs::{infra::db, web::routes};

use std::net::SocketAddr;
use std::path::PathBuf;

/// 默认端口刻意**不是 8080**：Kotlin 版还在那儿服务（灰度期两端并存）。
const DEFAULT_PORT: u16 = 8081;

struct Args {
    db: PathBuf,
    /// 产物目录（`<storage>/media` 的父目录）。默认在库文件旁边的 `storage/`。
    storage: PathBuf,
    host: String,
    port: u16,
    /// **父进程（桌面 App）没了就自己退** —— 见 [`spawn_stdin_watchdog`]。
    watch_stdin: bool,
    /// `--merge-library`：不是起服务，而是把别的库合并进 `--db` 指的这一份（见
    /// [`library_merge`]）。给文档 / 脚本一个"只增不减"的合并入口。
    merge: Option<MergeArgs>,
}

/// `--merge-library <源数据目录>` 那一组参数。
struct MergeArgs {
    sources: Vec<PathBuf>,
    media_from: Vec<PathBuf>,
    backup_dir: Option<PathBuf>,
    dry_run: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut db: Option<PathBuf> = None;
    let mut storage: Option<PathBuf> = None;
    let mut watch_stdin = false;
    let mut merge_sources: Vec<PathBuf> = Vec::new();
    let mut media_from: Vec<PathBuf> = Vec::new();
    let mut backup_dir: Option<PathBuf> = None;
    let mut dry_run = false;
    // 缺省监听回环。`COMFYHUB_ALLOW_REMOTE=1`（App 的「允许手机 / 局域网设备连接」
    // 开关就是走它）时听 `0.0.0.0`：手机在局域网里才有得连（用户 bug 2026-09-28
    // 「手机连接桌面端失败，报 timed out」—— `server.ps1` 以前硬编码 127.0.0.1，
    // 真机连桌面永远超时）。远程访问由配对闸门把关（回环放行、局域网要令牌）。
    let mut host = if remote_allowed() {
        "0.0.0.0".to_string()
    } else {
        "127.0.0.1".to_string()
    };
    let mut port = DEFAULT_PORT;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--db" => db = it.next().map(PathBuf::from),
            "--storage" => storage = it.next().map(PathBuf::from),
            "--host" => host = it.next().unwrap_or(host),
            // `--watch-stdin`：**由桌面 App 启动时必带**（见函数头）。父进程一死，
            // 我们这侧的 stdin 管道立刻回 0，于是优雅退出 —— 它取代了以前的
            // `scripts\watch-owner.ps1`（那个守护脚本随"App 直接起后端"一起删了）。
            "--watch-stdin" => watch_stdin = true,
            // `--merge-library <源数据目录>`（可给多次）：把别的库里的用户内容合并进
            // `--db` 那一份，合并完就退出（不起服务）。见 `infra::library_merge`。
            "--merge-library" => {
                let dir = it.next().ok_or("--merge-library 需要一个数据目录")?;
                merge_sources.push(PathBuf::from(dir));
            }
            // 文件已经不在源库的 `storage/` 里（构建残留、备份盘）时，告诉合并器去哪儿找
            "--media-from" => {
                let dir = it.next().ok_or("--media-from 需要一个目录")?;
                media_from.push(PathBuf::from(dir));
            }
            "--backup-dir" => {
                let dir = it.next().ok_or("--backup-dir 需要一个目录")?;
                backup_dir = Some(PathBuf::from(dir));
            }
            "--dry-run" => dry_run = true,
            "--port" => {
                port = it
                    .next()
                    .and_then(|p| p.parse().ok())
                    .ok_or("--port 需要一个数字")?
            }
            "--help" | "-h" => {
                println!(
                    "用法: comfyhub-server-rs [--db <路径>] [--storage <目录>] [--host <地址>] [--port <端口>] [--watch-stdin]\n\
                     \x20      comfyhub-server-rs --merge-library <源数据目录> [--merge-library …] \\\n\
                     \x20          [--media-from <目录>] [--db <目标库>] [--storage <目标 storage>] [--backup-dir <目录>] [--dry-run]\n\
                     \n\
                     合并只增不减：源库只读打开，目标库先备份，外键不自洽就整体回滚。"
                );
                std::process::exit(0);
            }
            other => return Err(format!("未知参数：{other}")),
        }
    }

    // 缺省库路径：`<可执行文件所在目录>/../.db/comfyhub.db`（发布包布局，AGENTS §7）
    let db = db.unwrap_or_else(|| {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        exe_dir.join("..").join(".db").join("comfyhub.db")
    });
    // 缺省产物目录：与库同级的 `storage/`（发布包布局，AGENTS §7）
    let storage = storage.unwrap_or_else(|| {
        db.parent()
            .and_then(|p| p.parent())
            .map(|root| root.join("storage"))
            .unwrap_or_else(|| PathBuf::from("storage"))
    });
    Ok(Args {
        db,
        storage,
        host,
        port,
        watch_stdin,
        merge: if merge_sources.is_empty() {
            None
        } else {
            Some(MergeArgs {
                sources: merge_sources,
                media_from,
                backup_dir,
                dry_run,
            })
        },
    })
}

/// **"我爸（App）还活着吗"** —— 靠 stdin 这根管道的生死判断。
///
/// # 为什么是 stdin，而不是 `--owner-pid` + `OpenProcess`
///
/// 桌面 App 用 `Process.start`（Dart）起我们时，**只**给我们继承三根管道，stdin 是
/// 其中一根，而它的写端**只有 App 持有**（Dart 侧 `CreateProcessPipe(stdin, kInheritRead)`
/// 之后就把子进程那头的读端留给我们、写端留在自己手里，并且用
/// `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` 保证不会被别的子孙进程继承）。于是：
///
/// | 事件 | 我们这一侧看到什么 |
/// | --- | --- |
/// | App 正常退出（先 `kill` 我们） | 不用等 watchdog，进程已经被杀了 |
/// | App 被任务管理器 / 崩溃强杀 | 写端随进程消失 → `read` 立刻回 `0` → 我们自己退 |
/// | App 还活着 | 管道开着，`read` 一直阻塞（不占 CPU） |
///
/// 换成 `--owner-pid` 就得再引一个 Win32 依赖（`OpenProcess` + `WaitForSingleObject`），
/// 而这件事用标准库一根管道就够了 —— 与"发布包要便携、少一个依赖少一份 CVE 面"的纪律一致。
///
/// ⚠ **只有"确实会给我们一根活管道"的调用方才许带 `--watch-stdin`**：
/// 从终端手动跑（stdin 是控制台，不是管道）时它读的是键盘，Ctrl+C / 关终端才算 EOF；
/// 用 `< NUL` 或 `Start-Process` 起（stdin 直接就是 EOF）时**会立刻自己退**，那是预期行为，
/// 但别拿它当"后台常驻"的开关。Posix 上同理（stdin 被重定向到 /dev/null 就是 EOF）。
fn spawn_stdin_watchdog() -> tokio::sync::mpsc::Receiver<()> {
    use std::io::Read as _;

    let (tx, rx) = tokio::sync::mpsc::channel::<()>(1);
    std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        loop {
            match std::io::stdin().read(&mut buf) {
                // 0 = EOF（写端全关了）；Err = 管道坏了。两种都说明父进程没了。
                Ok(0) => break,
                Ok(_) => {} // 有人往 stdin 打字/写东西：还活着，继续等
                Err(_) => break,
            }
        }
        let _ = tx.blocking_send(());
    });
    rx
}

/// `COMFYHUB_ALLOW_REMOTE` 是否要求监听所有网卡（1 / true / yes / on 都算）。
fn remote_allowed() -> bool {
    matches!(
        std::env::var("COMFYHUB_ALLOW_REMOTE")
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// 远程模式下绑**双栈** socket。
///
/// Windows 上 `0.0.0.0` 只收 IPv4；而校园 / 家庭网络经常把 IPv4 在 AP 之间隔离、
/// 只留 IPv6 能通（2026-11-08 实测：同一 SSID、不同 AP，手机 ping 不到桌面的
/// IPv4，但 `ping6` 通、HTTP 也通）。`[::]` + `only_v6(false)` 一个 socket 同时收
/// IPv4（映射成 `::ffff:…`）与 IPv6；桌面 App 仍连 `127.0.0.1`，配对闸门的
/// `is_loopback` 已认 `::ffff:127.0.0.1`。
fn bind_dual_stack(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    socket.set_only_v6(false)?;
    let addr: SocketAddr = format!("[::]:{port}")
        .parse()
        .unwrap_or_else(|_| panic!("IPv6 地址不合法：{port}"));
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    socket.set_nonblocking(true)?;
    let std_listener: std::net::TcpListener = socket.into();
    tokio::net::TcpListener::from_std(std_listener)
}

/// 绑监听：远程模式且 host 是"全部网卡"时走双栈，否则按用户给的地址原样绑。
async fn bind_listener(
    host: &str,
    port: u16,
    dual_stack: bool,
) -> std::io::Result<tokio::net::TcpListener> {
    if dual_stack {
        return bind_dual_stack(port);
    }
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .unwrap_or_else(|_| panic!("地址不合法：{host}:{port}"));
    tokio::net::TcpListener::bind(addr).await
}

#[tokio::main]
async fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(message) => {
            eprintln!("参数错误：{message}");
            std::process::exit(2);
        }
    };

    // `--merge-library`：合并不起服务（**先备份、再一个事务、外键不自洽整体回滚**）
    if let Some(merge) = args.merge {
        let request = comfyhub_server_rs::infra::library_merge::MergeRequest {
            target_db: args.db.clone(),
            target_storage: args.storage.clone(),
            sources: merge.sources,
            media_from: merge.media_from,
            backup_dir: merge.backup_dir,
            dry_run: merge.dry_run,
        };
        println!("目标库：{}", request.target_db.display());
        println!("目标产物目录：{}", request.target_storage.display());
        for source in &request.sources {
            println!("源数据目录：{}", source.display());
        }
        if request.dry_run {
            println!("演练模式（--dry-run）：只报告，不写库、不搬文件");
        }
        match comfyhub_server_rs::infra::library_merge::merge(&request) {
            Ok(report) => {
                print!("{}", report.render());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("合并失败：{e}");
                std::process::exit(2);
            }
        }
    }

    // 开库（**必要时建库**）：切流之后 Rust 是唯一的后端，"第一次运行能自己建库"必须成立 ——
    // 以前的版本缺库直接 exit 3（"先让 Kotlin 起一次库"），那在发布包里等于永远起不来。
    let conn = match db::open_or_create(&args.db) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("打开/建库失败：{e}");
            std::process::exit(3);
        }
    };

    // 启动自检：把 PRAGMA 读回来核对（R1），FTS5 不可用也要当场看见（R14 的桌面版）
    let engine = db::engine_info(&conn).unwrap_or_else(|_| serde_json::json!({}));
    let schema = db::schema_version(&conn).unwrap_or(-1);
    println!(
        "comfyhub-server-rs 启动：db={} storage={} schemaVersion={} engine={}",
        args.db.display(),
        args.storage.display(),
        schema,
        engine
    );
    if engine.get("fts5").and_then(|v| v.as_bool()) != Some(true) {
        eprintln!("⚠ FTS5 不可用：bundled 引擎本应带它，请检查 rusqlite 的 features");
    }

    let media_dir = args.storage.join("media");
    if !media_dir.is_dir() {
        // 不自己建目录：写路径还没搬过来，建空目录只会掩盖"storage 指错了"
        eprintln!(
            "⚠ 产物目录不存在：{}（文件流端点会回 404；用 --storage 指定正确的 storage 目录）",
            media_dir.display()
        );
    }
    let state = routes::AppState::new(
        conn,
        args.db.to_string_lossy().to_string(),
        media_dir.clone(),
    );

    // ---- Skills 投放口的自动登记（AGENTS §10）--------------------------------
    //
    // 用户把 skill 文件夹 / `.md` 拷进 `<storage>\ai\skills` 就算装好：没有 frontmatter 的
    // 在这里补 `name` / `description`（正文不动）。**不能只在点刷新时才做** ——
    // 用户装完直接重启应用是常态，那时界面上会一直显示"缺少 description"。
    let auto = comfyhub_server_rs::ai::skills::auto_register(&state);
    if auto.registered > 0 {
        println!(
            "Skills 自动登记：{} 个（{}）",
            auto.registered,
            auto.names.join("、")
        );
    }
    for error in &auto.errors {
        eprintln!("Skills 自动登记：{error}");
    }

    // ---- 启动恢复（与 Kotlin `Application.module` 的三步同序）----------------
    //
    // 这三步都是"上次进程死得不明不白"留下的残局，**少一步就是界面永远转圈**：
    // ① 还挂在 `running` 的 Run → `failed`（否则 SSE 一直等一条永远不来的终态事件）；
    // ② 还挂着 `pending` 的审批行 → `denied`（内存闸门随进程消失，那些审批
    //    "永远不会有人点"，而 `/api/ai/pending` 会一直把它们列出来）；
    // ③ 还挂着 `pending` 的提问行 → `expired`（同上）。
    match comfyhub_server_rs::ai::runs::fail_stale_running(&state, "后端重启导致 Run 中断")
    {
        Ok(n) if n > 0 => println!("启动恢复：{n} 个僵死 Run 已标为 failed"),
        Ok(_) => {}
        Err(e) => eprintln!("启动恢复：收尾僵死 Run 失败（{}）", e.human()),
    }
    match comfyhub_server_rs::ai::runs::expire_stale_pending_tool_calls(&state) {
        Ok(n) if n > 0 => println!("启动恢复：{n} 条待审批调用按拒绝收尾"),
        Ok(_) => {}
        Err(e) => eprintln!("启动恢复：收尾待审批调用失败（{}）", e.human()),
    }
    match comfyhub_server_rs::ai::questions::expire_stale(&state) {
        Ok(n) if n > 0 => println!("启动恢复：{n} 条待回答提问按失效收尾"),
        Ok(_) => {}
        Err(e) => eprintln!("启动恢复：收尾待回答提问失败（{}）", e.human()),
    }
    // ④ M5（契约 §4.1）：**手机排上来的提交**（`submit_jobs` 那张表）。
    //    它有三种残局，各有各的处置（见 `submit_queue::resume_in_flight` 的表）：
    //    还在等人批的按拒绝收尾（与上面第 ② 步**同一语义**）、人已经批过的复起提交、
    //    提交中途没了的**如实说"结果不确定"且不重投**。三种都**不许静默**。
    let queued = comfyhub_server_rs::mobile::submit_queue::resume_in_flight(&state);
    if queued > 0 {
        println!("启动恢复：{queued} 条排队提交被处置（审批被打断 / 提交被中断 / 复起）");
    }

    // ---- 附件缓存（2026-11-04）：清掉没人引用的裁图 / 搜图 / 上传 -------------
    //
    // 这些图是**真附件**（提问卡要能重开、历史要能显示），所以不能当临时文件删；
    // 但也不能无限长。规则见 `media::attachments` 顶部：被引用的一个都不删，
    // 没引用的留 7 天，总量超 2GB 再从最老的没引用图淘汰到 80%。
    match comfyhub_server_rs::media::attachments::collect_garbage(&state) {
        Ok(report) if report.removed > 0 || report.orphan_files > 0 => println!(
            "附件缓存：清掉 {} 张没被引用的图（{:.1} MB）、孤儿文件 {} 个；剩 {} 张 / {:.1} MB",
            report.removed,
            report.freed_bytes as f64 / 1024.0 / 1024.0,
            report.orphan_files,
            report.remaining,
            report.remaining_bytes as f64 / 1024.0 / 1024.0,
        ),
        Ok(_) => {}
        Err(e) => eprintln!("附件缓存清理失败（不影响使用）：{}", e.human()),
    }

    // ---- 老提示词的 LoRA 回填（2026-10-23）----------------------------------
    //
    // 2026-10-22 修了第三方 LoRA 加载节点（rgthree 的 `lora_01` / 嵌套对象等）的识别，
    // 但**已经入库**的 `prompts.loras` 不会自己变 —— 用户重开 App 看到的还是"少几条"。
    // 这里按回填版本跑一次：只补"入库后没被用户编辑过"的行里缺的 LoRA 名字，不改已有条目
    // （四条纪律见 `capture_ingest::backfill_loras`）。同步跑、不塞后台：
    // 界面首次拉列表时就必须是补好的。
    match state.write(|tx| {
        if !comfyhub_server_rs::comfy::capture_ingest::loras_backfill_needed(tx)? {
            return Ok(0);
        }
        Ok(comfyhub_server_rs::comfy::capture_ingest::run_loras_backfill(tx)?)
    }) {
        Ok(n) if n > 0 => {
            println!("[capture] LoRA 回填：{n} 条提示词补上了第三方加载节点里的 LoRA")
        }
        Ok(_) => {}
        Err(e) => eprintln!("[capture] LoRA 回填失败（不影响使用）：{}", e.human()),
    }

    // ---- 老提示词的"运行时正向提示词"回填（2026-10-25）----------------------
    //
    // Krea2 r2i 的 `TextGenerate` 在跑的时候才写出最终提示词，节点图里只有 system prompt
    // 与空槽；2026-10-25 之前入库的行因此正向是空的（`test_00901_.png`）。
    // 那次运行的 `/history` 条目（`capture_runs.raw`）里其实有最终文本，这里补一遍。
    // 只补空正向的行、只认"喂给了正向条件"的文本（见 `capture_ingest::backfill_runtime_prompts`）。
    match state.write(|tx| {
        if !comfyhub_server_rs::comfy::capture_ingest::prompt_backfill_needed(tx)? {
            return Ok(0);
        }
        Ok(comfyhub_server_rs::comfy::capture_ingest::run_prompt_backfill(tx)?)
    }) {
        Ok(n) if n > 0 => {
            println!("[capture] 提示词回填：{n} 条提示词补上了运行时生成的正向提示词")
        }
        Ok(_) => {}
        Err(e) => eprintln!("[capture] 提示词回填失败（不影响使用）：{}", e.human()),
    }

    // ---- 后台捕获轮询器（P4 第 47 步 = O5/O6）-------------------------------
    //
    // 与 Kotlin `ComfyCapture.start()` 同一件事：**用户自己在 ComfyUI 里跑的东西也要被收**。
    // 没有它时只有端点/工具触发一次，"AI 说生成了、画廊里却没有"就永远差这一半。
    // 它同时是 `/api/capture/status` 里 `lastPollAt` / `lastError` 的真源。
    state.capture_poller.clone().spawn(state.clone());

    // ---- 实时进度 WebSocket（P4 第 72 步）----------------------------------
    //
    // REST 拿不到进度（2026-09-27 实测：`/queue` 没有、`/progress` 404、`/history`
    // 只有跑完的），生成任务的 percent 与"当前步骤"全在这条 WS 上。
    // **常驻 + 重连**，与上面的轮询器同一条纪律：绝不在每个请求里现连一次
    // （那既收不到已经发出去的帧，也会把 ComfyUI 按 clientId 记的 socket 表撑爆）。
    comfyhub_server_rs::comfy::comfy_progress::spawn(&state.comfy_progress, state.clone());

    // ---- MCP 桥预热（P4 第 45 步）------------------------------------------
    //
    // 与 Kotlin `Application.module` 起 comfy-mcp 子进程同一件事：**在后台**拉起并拉清单，
    // 不阻塞端口监听（握手实测约 3 秒，等它会让 App 的"等待后端"多等三秒）。
    // 拉不到只记一句 —— 少一批工具也比后端起不来强。
    {
        let registry = state.mcp.clone();
        let project_root = state.project_root();
        tokio::spawn(async move {
            comfyhub_server_rs::tools::mcp::warmup(&registry, &project_root).await;
        });
    }

    // ---- ComfyUI `/object_info` 预热 ---------------------------------------
    //
    // 它是**兆字节级**响应，蓝图 / 配方 / 体检 / 盘查都要它。以前每开一次蓝图重新拉一遍
    // + 重新解析一遍，于是"点开蓝图"明显比 ComfyUI 自己慢（用户 2026-10-18）。
    // 这里在后台先取一次把缓存填上：ComfyUI 没起时**静默失败**（不缓存失败），
    // 下次真正要用时会照常重试 —— 所以这条预热永远不会让状态变坏。
    {
        let state = state.clone();
        tokio::spawn(async move {
            let _ = comfyhub_server_rs::workflow::workflow_recipe::object_info_of(&state).await;
        });
    }

    // ---- 手机排队提交的清扫循环（M5，契约 §4.1）-----------------------------
    //
    // 它同时干三件事，都是"受理了但还没跑完不会丢"的兜底：
    //  ① **看门狗**：worker 猝死留下的 `awaiting_approval` / `submitting` 按**租约**
    //     （`heartbeat_at` 比现在早过 `LEASE_SECONDS`）收成终态 —— 绝不让手机上那条
    //     "已排队"永远停在排队中；
    //  ② **孤儿复起**：人已经批过、但当时没人认领的（认领是原子的，复起不会重复提交）；
    //  ③ **回执投递**：终态的回执送不出去（会话还没同步上来 / 写库那次失败）就重试，
    //     `reply_state` + 确定性的 `reply_message_id` 保证"一定送到、且只送一条"。
    comfyhub_server_rs::mobile::submit_queue::spawn_sweeper(state.clone());

    // ---- 自己受限不自知（2026-09-29 引入；2026-09-30 机理订正为"完整性标签"）----
    //
    // 后端若跑在**低完整性（Low IL）令牌**下（工作区产物带着 Low 标签 + min(父进程, 映像)），
    // `comfy_start` 写不进 ComfyUI 的 `temp/`、`comfy_use_media` 写不进 input…，
    // 而**探端口/健康是看不出来的**。这里启动时探一次并留证（`.run\confinement.json`），
    // `GET /api/system/confinement` 会把同一份结论连判据一起交给界面（App 顶部常驻横幅）。
    // 探不到工作区（还没配 ComfyUI）不是毛病，不留证。
    {
        let state = state.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            comfyhub_server_rs::infra::confinement::refresh(&state);
        });
    }

    // ---- 产物**集合**的存量归并（2026-10-11）---------------------------------
    //
    // 老库刚补上 `collection_id` 那一列时**全是 NULL**，不跑这一趟就"一个集合都没有"，
    // 界面上表现为"这个功能好像没做"（而且不报错）。
    //
    // 四条纪律：
    //  ① **只补 `collection_id IS NULL` 的行** —— 用户手动拆开/合并过的（`origin='manual'`）
    //     和已经分好组的一律不碰（`collection::backfill` 里守着这条）；
    //  ② **只有"同款 >= 2 张"才算活**（独苗不成组）—— 探针 `pending_grouped` 与回填
    //     **共用同一份分桶**，所以下面那个数字就是"马上要做的事"，不是估算；
    //  ③ **有活才跑、才打；没活一行都不打**（别打"已分 0 张"那种废话）；
    //  ④ **收尾一定要跑**（与有没有活无关）—— 见下面「收尾」那一段。
    // 跑在后台线程里、不阻塞端口监听 —— 它可能要给几万条产物算 key。
    {
        let state = state.clone();
        std::thread::spawn(move || {
            // `state.with` 的回调只认 `rusqlite::Result`，而归并 / 收尾报的是
            // `WriteError`（端点层才需要那几档区分）。这里只是启动期的一趟，把错因
            // 转成文字塞回 rusqlite 就行。
            fn as_sql(e: comfyhub_server_rs::library::repo::WriteError) -> rusqlite::Error {
                rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                    e.to_string(),
                )))
            }

            let pending = match state.with(|conn| {
                comfyhub_server_rs::media::collection::pending_grouped(conn).map_err(as_sql)
            }) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!(
                        "[collection] 统计待归并产物失败（不影响使用）：{}",
                        e.human()
                    );
                    return;
                }
            };

            // 归并（有活才跑、才打：`pending` 就是"几组 / 几张"）。
            if pending.0 > 0 {
                match state.with(|conn| {
                    comfyhub_server_rs::media::collection::backfill(conn).map_err(as_sql)
                }) {
                    // 探针与回填**同一份分桶**，所以回填的返回值恒等于 `pending.1`
                    // （真差了这个数只可能是中途出错，那会走下面的 Err 分支）——
                    // 日志里就不再重复计一个计数器了。
                    Ok(_) => println!("[collection] 存量归并：{} 组 / {} 张", pending.0, pending.1),
                    // 归并失败**不拦启动**：集合只是分组视图，缺了不影响产物本身
                    Err(e) => eprintln!("[collection] 存量归并失败（不影响使用）：{}", e.human()),
                }
            }

            // 收尾：解散"`auto` 且只剩 1 个成员"的集合 + 清掉 0 成员的空壳
            //    （用户 2026-10-18 报的"旧集合元素为 1，旧集合没有自动解散"）。
            //
            //    ⚠ **必须在归并之后**：归并把产物挂进集合，收尾看的是挂完之后还剩几个成员。
            //    ⚠ 而且**不管有没有归并的活都要跑**，三个理由：
            //       ① 历史库里遗留的单成员 auto 组，归并永远不碰它（成员都还有归属）；
            //       ② 这一趟归并**没有包在事务里**（`state.with` 是逐条自动提交），
            //          中途中断可能留下半截的单成员组；
            //       ③ 顺带清掉点不开的空壳。
            match state.with(|conn| {
                comfyhub_server_rs::media::collection::prune_singleton_auto(conn).map_err(as_sql)
            }) {
                Ok(n) if n > 0 => println!("[collection] 解散单成员自动集合：{n} 个"),
                Ok(_) => {}
                Err(e) => {
                    eprintln!(
                        "[collection] 解散单成员集合失败（不影响使用）：{}",
                        e.human()
                    )
                }
            }
        });
    }

    let app = routes::router(state.clone())
        .layer(tower_http::cors::CorsLayer::permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let requested_port = args.port;
    // 候选端口被占 / 落在 Windows 保留段（Hyper-V/WSL 预留，netstat 都查不出来）时
    // **不报错退出**，改成让操作系统挑一个（Kotlin `ApiPortFile` 的那条纪律）。
    // 远程模式（默认 host=0.0.0.0）改走双栈：见 bind_dual_stack 的注释。
    let dual_stack = remote_allowed() && matches!(args.host.as_str(), "0.0.0.0" | "::" | "[::]");
    let mut fell_back = false;
    let (addr, listener) = match bind_listener(&args.host, requested_port, dual_stack).await {
        Ok(listener) => {
            // ⚠ **实际端口从 listener 读回来**，不能用 `requested_port`：
            // 用户给 `--port 0`（"让系统挑一个"）时，那个 0 不是端口 ——
            // 写进端口文件的是 0，App 就会去连 `http://127.0.0.1:0`（实测踩到）。
            let addr = listener.local_addr().expect("本地地址");
            (addr, listener)
        }
        Err(error) => {
            eprintln!(
                "绑定 {}:{} 失败（{error}），回落到系统分配的端口",
                args.host, requested_port
            );
            fell_back = true;
            match bind_listener(&args.host, 0, dual_stack).await {
                Ok(listener) => {
                    let addr = listener.local_addr().expect("本地地址");
                    (addr, listener)
                }
                Err(error) => {
                    eprintln!("回落到随机端口也失败：{error}");
                    std::process::exit(5);
                }
            }
        }
    };
    let actual_port = addr.port();
    let url = comfyhub_server_rs::web::api_port_file::url_for(&args.host, actual_port);
    println!("监听 {url}（pid={}）", std::process::id());
    if remote_allowed() {
        println!("允许局域网访问（COMFYHUB_ALLOW_REMOTE）：手机 / 其它设备用这台电脑的局域网 IP 连接；远程请求受配对闸门约束（回环放行，局域网要配对令牌）");
    }

    // 把**实际**端口告诉 AppState：`GET /api/net/urls` 用它给手机拼候选地址
    // （不能拿请求里的 Host —— 那是客户端走过的路，手机问的恰恰是"还有别的没有"）。
    state.set_port(actual_port);

    // 只监听回环时不要广播：手机连不上，广播只会让发现列表出现一个死地址。
    if args.host == "0.0.0.0" || dual_stack {
        comfyhub_server_rs::mobile::discovery::spawn(actual_port);
    }

    // **端口只有一个真源**：后端绑好之后写进 `<项目根>\.run\api.json`，
    // 脚本（`scripts\api-port.ps1`）、App（`settings_store.dart`）与 ComfyUI 捕获扩展都只读它。
    // ⚠ 这是"把 `server.ps1` 指向 Rust"的硬前提：不写它，App 会对着旧端口等 90 秒。
    // `COMFYHUB_NO_PORT_FILE=1` 只给自己的验收脚本用（跑真库副本时别去覆盖真后端那份记录）。
    let port_state = if std::env::var("COMFYHUB_NO_PORT_FILE")
        .map(|v| v.trim() == "1")
        .unwrap_or(false)
    {
        eprintln!("[api-port] COMFYHUB_NO_PORT_FILE=1：跳过写端口文件（自测用）");
        None
    } else {
        let project_root = state.project_root();
        let state_for_file = comfyhub_server_rs::web::api_port_file::ApiPortState {
            port: actual_port,
            pid: std::process::id(),
            host: args.host.clone(),
            url: url.clone(),
            started_at: comfyhub_server_rs::infra::iso_time::from_system_time(
                std::time::SystemTime::now(),
            ),
            requested_port,
            fell_back,
        };
        let written = comfyhub_server_rs::web::api_port_file::write(&project_root, &state_for_file);
        written.map(|_| (project_root, state_for_file))
    };

    // ⚠ **必须带 connect info**：配对闸门就是靠"来源地址是不是回环"决定放不放行的
    // （`docs/pairing-design.md` §11.4）。漏了它，每一个请求都会走"拿不到来源 = 非回环"
    // 那条保守分支 —— 闸门一开，本机的桌面 App 也会被自己关在门外。
    let shutdown_state = port_state.clone();

    // ---- 父进程退出就跟着退（2026-09-30：取代 scripts\watch-owner.ps1）-------
    //
    // 桌面 App 现在是**它自己**直接起我们（`lib\core\backend_launcher.dart`），
    // 不再经过 pwsh / WMI，所以"谁起的谁关"这件事必须我们自己兜住：
    // 正常关窗时 App 会先 `kill` 我们；**强杀 / 崩溃**那条路上没人来杀，
    // 靠的就是这根 stdin 管道（见 [`spawn_stdin_watchdog`] 的说明）。
    //
    // 走**优雅退出**而不是 `std::process::exit`：端口记录 `.run\api.json` 的收尾、
    // 在飞的请求都要走完（`delete_if_owned` 只删自己那一份，见 `api_port_file.rs`）。
    let mut owner_rx = if args.watch_stdin {
        Some(spawn_stdin_watchdog())
    } else {
        None
    };

    if let Err(e) = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        // 没开 watchdog 时这一支永远不完成（`pending`），只剩 Ctrl+C 一条路。
        let owner = async {
            match owner_rx.as_mut() {
                Some(rx) => {
                    let _ = rx.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => println!("收到停止信号，正在退出"),
            _ = owner => println!("父进程已退出（stdin 关闭），随之退出"),
        }
    })
    .await
    {
        eprintln!("服务异常退出：{e}");
        std::process::exit(6);
    }
    // 优雅退出时清掉端口记录，但**只清自己那一份**（重启时新进程刚写的那份不能被旧进程删掉）
    if let Some((project_root, state_for_file)) = shutdown_state {
        comfyhub_server_rs::web::api_port_file::delete_if_owned(&project_root, state_for_file.pid);
    }
}
