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
//! | `GET /api/{prompts,media}/{id}/workflow`、`GET /api/prompts/{id}/api-graph` | ✅ |
//! | **`/api/sync/{manifest,changes,push,bootstrap}`**（4 条全落地） | ✅ |
//! | `GET /api/ai/providers`（不含密钥）、`GET /api/ai/providers/{id}/models` | ✅ |
//! | **Provider 写侧**：`POST/PUT/DELETE /api/ai/providers`（乐观锁 AIH-007）、`PUT …/models` | ✅（P4 第 37 步） |
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
//! | **`/api/auth*` 配对闸门 + `GET /api/ai/credentials` 密钥下行**（inbound auth P0 + 配对第 2 步） | ✅ |
//! | 扫码配对（`POST /api/auth/pair/start`） | ⛔ **501**（本轮只做"手机发起 + 桌面显示 6 位码"，见 `docs/pairing-design.md` §11.1） |
//! | `POST /api/ai/capabilities/{refresh,probe}` | ⛔ **501**：要 models.dev 在线数据集 / 要真上游请求（会花钱），不满足"不需要人工校验"这条 |
//! | **上游 LLM 客户端 / 协议适配器**（第 29 步）与**主循环**（第 30 步） | ✅ 已落地（不带工具、不带附件）；工具层已搬四个本机工具 + 两个闸门（第 31/32 步） |
//! | **桌面文件工具五件**（`list_dir` / `read_file` / `write_file` / `register_skill` / `delete_skill`）+ **路径准入**（`resolve_read` / `resolve_write`：禁段、真实路径、目录根不可写） | ✅（P4 第 40 步；`desktop_tools.rs` + `policy.rs`，桌面提示词与下发的 `tools[]` 现在含它们） |
//! | **comfy 五件**：`comfy_get_run` / `comfy_list_workflows` / `comfy_get_status` / `comfy_sync_history`（写库：建提示词 + 产物入库）/ `comfy_load_workflow`（把本机工作流文件读进库，界面格式按 `/object_info` 转换） | ✅（P4 第 41~44 步） |
//! | **联网三工具**（`web_search` / `web_fetch` / `civitai_search`） | ⛔ **未搬**（系统提示里那一节暂时指向没下发的工具，见 `tools.rs` 的诚实清单） |
//! | **官方 comfy-mcp 那 39 个工具**（`system_stats` / `nodes` / `run_workflow` / `launch_comfyui` / `get_logs` …） | ✅（P4 第 45 步：`mcp.rs` 起 `comfy-mcp` 子进程走 JSON-RPC，清单进 `/api/ai/tools` 与下发的 `tools[]`；`launch_comfyui` 由我们**替用户补上共享模型根那三个参数**，失败时把 stderr 尾部带进 `detail`） |
//! | **`comfy_*` 其余八件**（`find_workflow` / `submit` / `use_attachment` / `check_inputs` / `fit_workflow` / `env_report` / `audit_workflow` / `prune_workflow`）+ `find_model` | ⛔ **未搬**（要 ComfyUI 在场 / 要模型清单，见 `tools.rs` 的诚实清单） |
//! | **`WorkflowConvert` 的两处欠账** | ⚠ **组节点（subgraph）不展开**、**「模拟用户行为」（`emulateFrontend`）没实现** —— 两者都**如实报 `unsupportedNodes`**（不猜）；工具描述是从 Kotlin 逐字搬的，所以结果里**主动补一句 warning** 说明这一步没做 |
//! | 附件（R9）：`/api/ai/attachments*` 与内联预算 | ⛔ **未搬**（有附件直接 `UNSUPPORTED_CONTENT`，不假装发得出去） |
//! | `/api/capture/{import,import-workflows}`、`/api/ingest/comfyui` | ⛔ **未搬**（回 **501**，不假装成功；`import-workflows` 要整套 `WorkflowConvert` 转换栈，`ingest` 要 ComfyUI 推送那条路） |
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

    if !args.db.exists() {
        // 不自己建库：schema 的真源在 Kotlin 侧（迁移/建表都归它）。
        // 灰度期"库还没建"是配置问题，要说清楚而不是造一个空库出来。
        eprintln!(
            "找不到数据库文件：{}\n（Rust 版**不建表**：先让 Kotlin 版起一次库，或用 --db 指定已有的 comfyhub.db）",
            args.db.display()
        );
        std::process::exit(3);
    }

    let conn = match db::open(&args.db) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("打开数据库失败：{e}");
            std::process::exit(4);
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

    let app = routes::router(state)
        .layer(tower_http::cors::CorsLayer::permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .unwrap_or_else(|_| panic!("地址不合法：{}:{}", args.host, args.port));
    println!("监听 http://{addr}（灰度期只提供已搬过来的只读端点）");

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("绑定 {addr} 失败：{e}");
            std::process::exit(5);
        }
    };
    // ⚠ **必须带 connect info**：配对闸门就是靠"来源地址是不是回环"决定放不放行的
    // （`docs/pairing-design.md` §11.4）。漏了它，每一个请求都会走"拿不到来源 = 非回环"
    // 那条保守分支 —— 闸门一开，本机的桌面 App 也会被自己关在门外。
    if let Err(e) = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            println!("收到停止信号，正在退出");
        })
        .await
    {
        eprintln!("服务异常退出：{e}");
        std::process::exit(6);
    }
}
