//! **真库副本抽查**（工具层专用）—— 默认 `#[ignore]`，只在显式指定时跑。
//!
//! # 为什么需要它
//!
//! 端点类的批次可以"起服务 + curl"在真库副本上抽查（P4 前 40 步都是那么做的）。
//! 但**工具层**没有 HTTP 入口：`comfy_get_run` / `comfy_list_workflows` /
//! 以后的 `comfy_submit` 都是"从执行器里被调到的"。只跑内存夹具的话，
//! 恰好会漏掉真库上那些"夹具里没有的行与列"（P4 第 40 步之前就栽过一次：
//! `ai_message_parts` 的复合主键在夹具里根本没有行）。
//!
//! 所以这里给工具层留一条**可复用的真库入口**：把库**副本**（绝不要对着真库跑，
//! 下面的工具里有会写盘的）与真实的 storage / 项目根指进来，直接调 `tools::execute`。
//!
//! # 怎么跑
//!
//! ```powershell
//! $spot = Join-Path $env:TEMP 'comfyhub-spot'          # 先建好并拷一份库进去
//! Copy-Item .db\comfyhub.db "$spot\comfyhub.db"
//! $env:COMFYHUB_SPOT_DB = "$spot\comfyhub.db"
//! $env:COMFYHUB_SPOT_STORAGE = "$PWD\storage"          # 只读用途；写类工具请指向副本
//! $env:COMFYHUB_SPOT_PROJECT_ROOT = "$PWD"
//! cargo test --manifest-path server-rs\Cargo.toml --test spot_check -- --ignored --nocapture
//! ```
//!
//! 没有设 `COMFYHUB_SPOT_DB` 时**直接跳过**（打印一句），不假装通过。

use comfyhub_server_rs::routes::AppState;
use comfyhub_server_rs::tools::{self, ToolSession};

/// 打开真库副本 + 真实 storage（没设环境变量就跳过）。
fn spot_state() -> Option<(AppState, std::path::PathBuf)> {
    let db = std::env::var("COMFYHUB_SPOT_DB").ok()?;
    if db.trim().is_empty() || !std::path::Path::new(&db).is_file() {
        eprintln!("跳过：COMFYHUB_SPOT_DB 没指向一个库文件（{db}）");
        return None;
    }
    let media = match std::env::var("COMFYHUB_SPOT_STORAGE") {
        Ok(storage) if !storage.trim().is_empty() => std::path::PathBuf::from(storage).join("media"),
        _ => std::path::Path::new(&db)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("storage")
            .join("media"),
    };
    let conn = comfyhub_server_rs::db::open(std::path::Path::new(&db)).expect("打开库副本");
    let state = AppState::new(conn, db.clone(), media);
    // 项目根决定白名单与"三份目录来源"里的第一份，必须与真机一致
    let root = std::env::var("COMFYHUB_SPOT_PROJECT_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| state.project_root());
    Some((state, root))
}

/// 调一个工具并把结果打出来（真库抽查的价值在**看得见的输出**，用例只做基本断言）。
async fn call(state: &AppState, name: &str, arguments: serde_json::Value) -> serde_json::Value {
    let policy = comfyhub_server_rs::policy::ToolPolicy::load(state).expect("读策略");
    let mut session = ToolSession::default();
    match tools::execute(state, &policy, "spot-run", "spot-conv", name, &arguments, &mut session).await
    {
        Ok(out) => {
            println!("── {name} OK ──\n{}", out.content);
            out.payload
        }
        Err(failure) => {
            println!("── {name} 失败 ──\n{}：{}", failure.code, failure.message);
            panic!("真库抽查里 {name} 报错了");
        }
    }
}

/// `comfy_list_workflows` + `comfy_get_run`：真库 + 真 ComfyUI 目录上抽查。
#[tokio::test]
#[ignore = "真库副本抽查：需要 COMFYHUB_SPOT_DB / COMFYHUB_SPOT_STORAGE"]
async fn real_db_spot_check_comfy_queries() {
    let Some((state, root)) = spot_state() else { return };
    println!("项目根={}（库=COMFYHUB_SPOT_DB）", root.display());

    // ① 列本机工作流：真机上应当能探到用户 ComfyUI 的 workflows 目录
    let listed = call(&state, "comfy_list_workflows", serde_json::json!({ "limit": 5 })).await;
    println!(
        "目录来源 {} 个，命中 {} 份（total {}）",
        listed["dirs"].as_array().map(Vec::len).unwrap_or(0),
        listed["count"],
        listed["total"]
    );
    assert!(listed["files"].is_array());

    // ③ 捕获状态（第 42 步）：真机上 ComfyUI 可能在也可能不在 —— **两种都要如实**
    let status = comfyhub_server_rs::capture::status_json(&state).await.expect("状态");
    println!("── /api/capture/status ──\n{status}");
    assert!(status["comfyReachable"].is_boolean());
    assert!(status["capturedRuns"].as_i64().unwrap_or(0) >= 0);
    // 连不上时队列数必须是 0（不许编一个数出来）
    if status["comfyReachable"] == false {
        assert_eq!(status["queueRunning"], 0);
        assert_eq!(status["queuePending"], 0);
    }

    // ⑤ 图解析对账（第 43 步第一半）：真库里的 `prompts` 行是 **Kotlin 的 GraphParse** 建的，
    //    而 `capture_runs.raw` 就是它当时的输入 —— 拿同一份输入在 Rust 侧再解析一遍、
    //    逐字段比，是"两端解析一致"唯一的实证（单测只能覆盖我想到的形状）。
    let mut mismatches: Vec<String> = Vec::new();
    let mut compared = 0usize;
    state
        .with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.id, p.positive_prompt, p.negative_prompt, p.checkpoint, p.sampler, \
                        p.scheduler, p.steps, p.cfg_scale, p.seed, p.width, p.height, p.batch_size, \
                        p.kind, c.raw \
                   FROM prompts p JOIN capture_runs c ON c.run_key = p.source_ref \
                  WHERE c.raw IS NOT NULL ORDER BY p.id DESC LIMIT 25",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<f64>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                ))
            })?;
            for row in rows {
                let (
                    prompt_id,
                    positive,
                    negative,
                    checkpoint,
                    sampler,
                    scheduler,
                    steps,
                    cfg,
                    seed,
                    width,
                    height,
                    batch,
                    kind,
                    raw,
                ) = row?;
                let Some(raw) = raw else { continue };
                let Ok(entry) = serde_json::from_str::<serde_json::Value>(&raw) else { continue };
                let graph = match comfyhub_server_rs::capture::history::parse(&entry).graph {
                    Some(serde_json::Value::Object(map)) => map,
                    _ => continue,
                };
                let parsed = comfyhub_server_rs::graph_parse::parse(Some(&graph));
                // 抽查时把前两条**逐字段打出来**：真库上不一致时，这几行就是第一现场
                if compared < 2 {
                    let mut classes: Vec<String> = graph
                        .iter()
                        .map(|(id, node)| {
                            format!(
                                "{id}:{}",
                                node.get("class_type").and_then(|v| v.as_str()).unwrap_or("?")
                            )
                        })
                        .collect();
                    classes.sort();
                    println!("  · prompt {prompt_id} 节点 {} 个：{}", graph.len(), classes.join(" "));
                    println!(
                        "    rust: positive={:?} negative={:?} seed={:?} steps={:?}",
                        parsed.positive,
                        parsed.negative.as_deref().map(|t| t.chars().take(40).collect::<String>()),
                        parsed.seed,
                        parsed.steps
                    );
                    println!(
                        "    kotlin: positive={:?} negative={:?} seed={:?} steps={:?}",
                        positive.as_deref().map(|t| t.chars().take(40).collect::<String>()),
                        negative.as_deref().map(|t| t.chars().take(40).collect::<String>()),
                        seed,
                        steps
                    );
                }
                compared += 1;
                let mut diff = |field: &str, mine: String, theirs: String| {
                    if mine != theirs {
                        mismatches.push(format!("prompt {prompt_id} {field}: rust={mine:?} kotlin={theirs:?}"));
                    }
                };
                diff("positive", parsed.positive.clone(), positive.clone().unwrap_or_default());
                diff(
                    "negative",
                    parsed.negative.clone().unwrap_or_default(),
                    negative.clone().unwrap_or_default(),
                );
                diff(
                    "checkpoint",
                    parsed.checkpoint.clone().unwrap_or_default(),
                    checkpoint.clone().unwrap_or_default(),
                );
                diff("sampler", parsed.sampler.clone().unwrap_or_default(), sampler.clone().unwrap_or_default());
                diff(
                    "scheduler",
                    parsed.scheduler.clone().unwrap_or_default(),
                    scheduler.clone().unwrap_or_default(),
                );
                diff("steps", format!("{:?}", parsed.steps), format!("{:?}", steps));
                diff("cfg", format!("{:?}", parsed.cfg), format!("{:?}", cfg));
                diff("seed", format!("{:?}", parsed.seed), format!("{:?}", seed));
                diff("width", format!("{:?}", parsed.width), format!("{:?}", width));
                diff("height", format!("{:?}", parsed.height), format!("{:?}", height));
                diff("batch", format!("{:?}", parsed.batch), format!("{:?}", batch));
                diff("kind", parsed.kind.clone(), kind.clone().unwrap_or_default());
            }
            Ok(())
        })
        .unwrap();
    println!("── 图解析对账 ── 比了 {compared} 条（prompts.source_ref → capture_runs.raw）");
    for line in mismatches.iter().take(30) {
        println!("  ✗ {line}");
    }
    println!("  不一致 {} 处", mismatches.len());
    // ⚠ 用户改过提示词的行**本来就会不一致**（`prompts` 是用户可编辑的），
    // 所以这里只报数、不把"有不一致"当失败；真正要盯的是"解析器整个崩掉"或
    // "positive 大面积空掉"这种系统性偏差。
    assert!(compared > 0, "一条可比对的记录都没有 —— 抽查没生效");
    let empty_positive = mismatches
        .iter()
        .filter(|line| line.contains("positive: rust=\"\"") && !line.contains("kotlin=\"\""))
        .count();
    assert_eq!(empty_positive, 0, "Rust 解析出的正向提示词不该系统性为空：{mismatches:?}");

    // ④ 按 runKey 查一条**真库里存在**的捕获记录
    let run_key: Option<String> = state
        .with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT run_key FROM capture_runs WHERE run_key IS NOT NULL ORDER BY id DESC LIMIT 1",
                    [],
                    |row| row.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten())
        })
        .unwrap();
    match run_key {
        Some(key) => {
            let found = call(&state, "comfy_get_run", serde_json::json!({ "runKey": key })).await;
            assert_eq!(found["runKey"], key);
            assert!(found["status"].is_string());
            // 可空键**整个不出现**（Kotlin `explicitNulls = false`）
            for nullable in ["title", "error", "capturedAt", "promptId"] {
                if found.get(nullable).is_some() {
                    assert!(!found[nullable].is_null(), "{nullable} 出现了却是 null：{found}");
                }
            }
        }
        None => println!("真库里一条 capture_runs 都没有 —— 跳过这一半"),
    }
}
