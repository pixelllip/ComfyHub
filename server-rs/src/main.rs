//! ComfyHub 桌面服务端（**Rust 版**，P4 灰度中）。
//!
//! ```
//! comfyhub-server-rs --db D:\.db\comfyhub.db --port 8081
//! ```
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
//! | **`/api/sync/{manifest,changes,push,bootstrap}`**（4 条全落地） | ✅ |
//! | `GET /api/ai/providers`（不含密钥）、`GET /api/ai/providers/{id}/models` | ✅ |
//! | **Provider 写侧**：`POST/PUT/DELETE /api/ai/providers`（乐观锁 AIH-007）、`PUT …/models` | ✅（P4 第 37 步） |
//! | **Provider 连接测试 / 模型发现**：`POST /api/ai/providers/{id}/{test,discover-models}` | ✅（P4 第 53 步：`provider_probe.rs`；`/models` 的两种 Base URL 写法都试、404 不算鉴权失败、密钥脱敏、能力**逐维度**合并内置目录） |
//! | **凭据写侧**：`GET/PUT/DELETE /api/ai/providers/{id}/credentials`、`/api/ai/web-search/credential` | ✅（P4 第 34 步，DPAPI 与 Kotlin 共用一份文件） |
//! | **Skills 六条**：`/api/ai/skills{,/roots,/rescan,/{name}}` | ✅（P4 第 35 步） |
//! | **长期记忆五条**：`GET/PUT/DELETE /api/ai/memory`、`POST …/entries`、`POST …/delete` | ✅（P4 第 36 步） |
//! | **内置模型目录**：`GET /api/ai/builtin/status`、`POST /api/ai/builtin/sync` | ✅（P4 第 38 步） |
//! | **能力目录状态**：`GET /api/ai/capabilities/status`、`PUT …/probe-toggle` | ✅（P4 第 39 步） |
//! | `GET /api/ai/conversations`、`/{id}`、`/{id}/messages`（**读侧**：历史会话与有序块） | ✅ |
//! | **角色卡五条**：`/api/ai/character-cards{,/{id}}`（列表 / 新建 201 / 详情 / 全量编辑 / 删除） | ✅（P4 第 46 步：`character_cards.rs` 逐条对照 Kotlin `CharacterCardRepo`） |
//! | **资料库两件工具**：`search_character_cards` / `save_character_card`（写角色前先查本机） | ✅（P4 第 46 步；`tagsVerifiedAt` 本轮**如实留空**，等 `danbooru_lookup` 搬过来才记时间） |
//! | `POST/PATCH/DELETE /api/ai/conversations`、`POST …/messages`（**写侧**） | ✅ |
//! | **Run 生命周期**：`POST …/runs`（202）/ `GET /runs/{id}` / `GET /runs/{id}/events`（**SSE**）/ `POST /runs/{id}/cancel` | ✅（事件流按 `?after=` 从库里拉，见 `run_lifecycle.rs`） |
//! | **`GET /api/ai/run-kit{,/status}`**（手机 harness 的全部规则类输入；R12/R13 的第一次重建） | ✅ |
//! | **`/api/ai/pending`**（跨端待办总览：还在等人的提问与工具审批） | ✅ |
//! | **`/api/auth*` 配对闸门 + `GET /api/ai/credentials` 密钥下行 + `PUT` 密钥上行**（inbound auth P0 + 配对第 2 步 / §14） | ✅ 下行的开关是 `allowCredentials`、上行是**另一个**开关 `allowCredentialsUpload`（两个都默认关，§14.1）；上行按 `ai.credentials.rev` 做"等值才应用"（落后回 `STALE_IGNORED`），逐条独立（env 只读 → `FORBIDDEN`、坏名字/空值 → `BAD_PAYLOAD`） |
//! | 扫码配对（`POST /api/auth/pair/start`） | ✅（2026-09-27：**仅回环**。回 `ticket` + 6 位码 + `urls[]`（来自 `net_urls`）+ 两个过期时刻；二维码那串 URI 由**前端**编（`lib/core/pairing_payload.dart` 是格式的唯一实现）。扫码会话与"手机举手"共用同一套 `Session`，所以 `pair/claim` 两条路都认；`ticket` 只在这个响应里出现，不进 `pair/pending` —— 那张列表是给手敲那条路用的） |
//! | `POST /api/ai/capabilities/{refresh,probe}` | `refresh` **✅（2026-09-27）**：从 `https://models.dev/api.json` 拉在线数据集 → 规范化成覆盖层写 `<storage>\ai\capabilities\remote.json`（不重启、不发版）；失败**不抛错、旧目录原样保留**，结果用 `lastMessage` 如实回给界面；`COMFYHUB_NO_DOWNLOAD=1` 时如实说"已关闭在线更新"。`probe`（实测校准，会对每个模型发真请求、花钱）**仍 501** |
//! | **上游 LLM 客户端 / 协议适配器**（第 29 步）与**主循环**（第 30 步） | ✅ 已落地（不带工具、不带附件）；工具层已搬四个本机工具 + 两个闸门（第 31/32 步） |
//! | **桌面文件工具五件**（`list_dir` / `read_file` / `write_file` / `register_skill` / `delete_skill`）+ **路径准入**（`resolve_read` / `resolve_write`：禁段、真实路径、目录根不可写） | ✅（P4 第 40 步；`desktop_tools.rs` + `policy.rs`，桌面提示词与下发的 `tools[]` 现在含它们） |
//! | **comfy 五件**：`comfy_get_run` / `comfy_list_workflows` / `comfy_get_status` / `comfy_sync_history`（写库：建提示词 + 产物入库）/ `comfy_load_workflow`（把本机工作流文件读进库，界面格式按 `/object_info` 转换） | ✅（P4 第 41~44 步） |
//! | **`comfy_submit`（R10）**：审核 → 审批 → 提交 → **自己等** → `captureRun` 入库 → `mediaIds` | ✅（P4 第 48 步：`workflow_submit.rs`；参数覆盖 / 补线逐条移植，超时如实报 `timeout`；`/api/capture/jobs` 的 `submissions[]` 也由它维护。**第 70 步把闸门接全**：`workflow_audit::blocking_refusal` 那三项"必跑不了"（前端节点 / 版本不兼容 / 本机没有的节点类）当场拒（`MODEL_NOT_LOADABLE`，与 Kotlin 同码同措辞），参数 / 显存 / 提示词风格那些非阻断的走 `preflightWarnings` **只提醒不拦**） |
//! | **生成任务的实时进度 + 当前步骤**（`comfy_progress.rs`：常驻 `ws://<comfyUrl>/ws` + 指数退避重连；`/api/capture/{status,jobs}` 的 `current` 与 `submissions[].progress`、`comfy_get_status` 的 `current` / `currentStep` / `progressConnected`） | ✅（P4 第 72 步。REST 拿不到进度 —— 2026-09-27 实测 `/queue` 没有、`/progress` 404、`/history` 只有跑完的，所以这是唯一来源。`percent` **只**来自 ComfyUI 自己发的 `value/max`（`progress` / `progress_state` 帧），**拿不到就是 `percent` 缺席（进度未知）**，绝不用"已执行节点数/总节点数"冒充；终态也不把 percent 抬到 100） |
//! | **`GET /api/net/urls`**（这台桌面有哪些**可达地址**：回环 / 局域网 / Tailscale） | ✅（P4 第 73 步：`net_urls.rs`。给手机的「同步与连接」页当候选 —— 手机没法自己扫网段猜桌面地址，而"能不能连上"由客户端打一次真实请求决定，分类只用于显示。端口用**我们实际监听的**那个；`port == 0` 时如实回空数组，不编地址） |
//! | **联网三工具 + 定向查询**（`web_search` / `web_fetch` / `civitai_search` / `find_model`） | ✅（P4 第 64 步：`web_tools.rs` / `civitai.rs` / `find_model.rs`。四件都是 **WEB 档**，共用 `MAX_WEB_QUERIES_PER_RUN` 那份预算、出厂全 `ASK`；`web_fetch` 的目标过 SSRF 闸门，`find_model` **先查本机再查外网**且**绝不代下**） |
//! | **官方 comfy-mcp 那 39 个工具**（`system_stats` / `nodes` / `run_workflow` / `launch_comfyui` / `get_logs` …） | ✅（P4 第 45 步：`mcp.rs` 起 `comfy-mcp` 子进程走 JSON-RPC，清单进 `/api/ai/tools` 与下发的 `tools[]`；`launch_comfyui` 由我们**替用户补上共享模型根那三个参数**，失败时把 stderr 尾部带进 `detail`） |
//! | **工作流审核 + 摘拼**：`comfy_audit_workflow`（七项确定性检查）/ `comfy_prune_workflow`（可达性摘除 + 按 `/object_info` 拼装） | ✅（P4 第 63 步：`workflow_audit.rs` / `workflow_prune.rs`；两件都**只报不改**，改好的图是**内联**返回的） |
//! | **工作流配方三件**：`comfy_learn_workflow` / `comfy_list_recipes` / `comfy_bind_recipe` + `workflow_recipes` 表 + `/api/ai/workflow-recipes*` | ✅（P4 第 67 步：`workflow_recipe.rs` —— 归纳成「任务 + 骨架指纹 + 模型槽位 + 参数旋钮」，默认**拒学**转换有缺口的图（`UNSUPPORTED_NODES`），绑**只报不改**；学习与绑定与端点**共用一份实现**。**系统提示第 20 条**（P4 第 69 步）已按 Kotlin 原文补进常驻段，`PROMPT_VERSION` 随之 **v32 → v33**；正文真源是 `src/nsfw_prompt.rs`（`gen_prompts.py` 已停用）） |
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

use comfyhub_server_rs::{db, routes};

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
}

fn parse_args() -> Result<Args, String> {
    let mut db: Option<PathBuf> = None;
    let mut storage: Option<PathBuf> = None;
    let mut host = "127.0.0.1".to_string();
    let mut port = DEFAULT_PORT;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--db" => db = it.next().map(PathBuf::from),
            "--storage" => storage = it.next().map(PathBuf::from),
            "--host" => host = it.next().unwrap_or(host),
            "--port" => {
                port = it
                    .next()
                    .and_then(|p| p.parse().ok())
                    .ok_or("--port 需要一个数字")?
            }
            "--help" | "-h" => {
                println!(
                    "用法: comfyhub-server-rs [--db <路径>] [--storage <目录>] [--host <地址>] [--port <端口>]"
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
    Ok(Args { db, storage, host, port })
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

    // ---- 启动恢复（与 Kotlin `Application.module` 的三步同序）----------------
    //
    // 这三步都是"上次进程死得不明不白"留下的残局，**少一步就是界面永远转圈**：
    // ① 还挂在 `running` 的 Run → `failed`（否则 SSE 一直等一条永远不来的终态事件）；
    // ② 还挂着 `pending` 的审批行 → `denied`（内存闸门随进程消失，那些审批
    //    "永远不会有人点"，而 `/api/ai/pending` 会一直把它们列出来）；
    // ③ 还挂着 `pending` 的提问行 → `expired`（同上）。
    match comfyhub_server_rs::runs::fail_stale_running(&state, "后端重启导致 Run 中断") {
        Ok(n) if n > 0 => println!("启动恢复：{n} 个僵死 Run 已标为 failed"),
        Ok(_) => {}
        Err(e) => eprintln!("启动恢复：收尾僵死 Run 失败（{}）", e.human()),
    }
    match comfyhub_server_rs::runs::expire_stale_pending_tool_calls(&state) {
        Ok(n) if n > 0 => println!("启动恢复：{n} 条待审批调用按拒绝收尾"),
        Ok(_) => {}
        Err(e) => eprintln!("启动恢复：收尾待审批调用失败（{}）", e.human()),
    }
    match comfyhub_server_rs::questions::expire_stale(&state) {
        Ok(n) if n > 0 => println!("启动恢复：{n} 条待回答提问按失效收尾"),
        Ok(_) => {}
        Err(e) => eprintln!("启动恢复：收尾待回答提问失败（{}）", e.human()),
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
    comfyhub_server_rs::comfy_progress::spawn(&state.comfy_progress, state.clone());

    // ---- MCP 桥预热（P4 第 45 步）------------------------------------------
    //
    // 与 Kotlin `Application.module` 起 comfy-mcp 子进程同一件事：**在后台**拉起并拉清单，
    // 不阻塞端口监听（握手实测约 3 秒，等它会让 App 的"等待后端"多等三秒）。
    // 拉不到只记一句 —— 少一批工具也比后端起不来强。
    {
        let registry = state.mcp.clone();
        let project_root = state.project_root();
        tokio::spawn(async move {
            comfyhub_server_rs::mcp::warmup(&registry, &project_root).await;
        });
    }

    let app = routes::router(state.clone())
        .layer(tower_http::cors::CorsLayer::permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let requested_port = args.port;
    // 候选端口被占 / 落在 Windows 保留段（Hyper-V/WSL 预留，netstat 都查不出来）时
    // **不报错退出**，改成让操作系统挑一个（Kotlin `ApiPortFile` 的那条纪律）。
    let parse_addr = |port: u16| -> SocketAddr {
        format!("{}:{}", args.host, port)
            .parse()
            .unwrap_or_else(|_| panic!("地址不合法：{}:{}", args.host, port))
    };
    let mut fell_back = false;
    let (addr, listener) = match tokio::net::TcpListener::bind(parse_addr(requested_port)).await {
        Ok(listener) => {
            // ⚠ **实际端口从 listener 读回来**，不能用 `requested_port`：
            // 用户给 `--port 0`（"让系统挑一个"）时，那个 0 不是端口 ——
            // 写进端口文件的是 0，App 就会去连 `http://127.0.0.1:0`（实测踩到）。
            let addr = listener.local_addr().expect("本地地址");
            (addr, listener)
        }
        Err(error) => {
            eprintln!("绑定 {}:{} 失败（{error}），回落到系统分配的端口", args.host, requested_port);
            fell_back = true;
            match tokio::net::TcpListener::bind(parse_addr(0)).await {
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
    let url = comfyhub_server_rs::api_port_file::url_for(&args.host, actual_port);
    println!("监听 {url}（pid={}）", std::process::id());

    // 把**实际**端口告诉 AppState：`GET /api/net/urls` 用它给手机拼候选地址
    // （不能拿请求里的 Host —— 那是客户端走过的路，手机问的恰恰是"还有别的没有"）。
    state.set_port(actual_port);

    // **端口只有一个真源**：后端绑好之后写进 `<项目根>\.run\api.json`，
    // 脚本（`scripts\api-port.ps1`）、App（`settings_store.dart`）与 ComfyUI 捕获扩展都只读它。
    // ⚠ 这是"把 `server.ps1` 指向 Rust"的硬前提：不写它，App 会对着旧端口等 90 秒。
    // `COMFYHUB_NO_PORT_FILE=1` 只给自己的验收脚本用（跑真库副本时别去覆盖真后端那份记录）。
    let port_state = if std::env::var("COMFYHUB_NO_PORT_FILE").map(|v| v.trim() == "1").unwrap_or(false) {
        eprintln!("[api-port] COMFYHUB_NO_PORT_FILE=1：跳过写端口文件（自测用）");
        None
    } else {
        let project_root = state.project_root();
        let state_for_file = comfyhub_server_rs::api_port_file::ApiPortState {
            port: actual_port,
            pid: std::process::id(),
            host: args.host.clone(),
            url: url.clone(),
            started_at: comfyhub_server_rs::iso_time::from_system_time(std::time::SystemTime::now()),
            requested_port,
            fell_back,
        };
        let written = comfyhub_server_rs::api_port_file::write(&project_root, &state_for_file);
        written.map(|_| (project_root, state_for_file))
    };

    // ⚠ **必须带 connect info**：配对闸门就是靠"来源地址是不是回环"决定放不放行的
    // （`docs/pairing-design.md` §11.4）。漏了它，每一个请求都会走"拿不到来源 = 非回环"
    // 那条保守分支 —— 闸门一开，本机的桌面 App 也会被自己关在门外。
    let shutdown_state = port_state.clone();
    if let Err(e) = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            println!("收到停止信号，正在退出");
        })
        .await
    {
        eprintln!("服务异常退出：{e}");
        std::process::exit(6);
    }
    // 优雅退出时清掉端口记录，但**只清自己那一份**（重启时新进程刚写的那份不能被旧进程删掉）
    if let Some((project_root, state_for_file)) = shutdown_state {
        comfyhub_server_rs::api_port_file::delete_if_owned(&project_root, state_for_file.pid);
    }
}
