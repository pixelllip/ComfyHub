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
//! | `GET /api/{prompts,media}/{id}/workflow`、`GET /api/prompts/{id}/api-graph` | ✅ |
//! | **`/api/sync/{manifest,changes,push,bootstrap}`**（4 条全落地） | ✅ |
//! | `GET /api/ai/providers`（不含密钥）、`GET /api/ai/providers/{id}/models` | ✅ |
//! | `GET /api/ai/conversations`、`/{id}`、`/{id}/messages`（**读侧**：历史会话与有序块） | ✅ |
//! | `/api/ai/*` 的其余部分（会话写侧 / Run / SSE / run-kit / 工具 / 审批 / 附件） | ⛔ **未搬**（Kotlin 那边继续服务） |
//! | `/api/capture/{status,jobs,poll,import-workflows}`、`/api/ingest/comfyui` | ⛔ **未搬**（回 **501**，不假装成功） |
//! | R6~R10 的安全防线 | ⛔ **未重建**（AI/工具层是最后一批，总纲 §6 要求逐条对照重建） |

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
    if let Err(e) = axum::serve(listener, app)
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
