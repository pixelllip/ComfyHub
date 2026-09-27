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

/// **重放一次真实运行**（第 43 步第二半）：拿真库里那条 `capture_runs.raw`，
/// 在**副本**上重新走一遍入库链（解析 → 建提示词 → 收产物 → 收尾）。
///
/// 为什么值得单独跑：单测里的 `/history` 是手写的，而真库里的那条 raw 带着
/// **真实节点的怪癖**（数字引用、字符串 steps、共享模型根……）。这条路能证明
/// "解析 + 入库"两半在真数据上真的合得起来。
#[tokio::test]
#[ignore = "真库副本抽查：需要 COMFYHUB_SPOT_DB / COMFYHUB_SPOT_STORAGE"]
async fn real_db_spot_check_replay_ingest() {
    let Some((state, _root)) = spot_state() else { return };
    // 挑一条**有产物**的真记录（最近的那条）
    let picked: Option<(String, String)> = state
        .with(|conn| {
            use rusqlite::OptionalExtension;
            Ok(conn
                .query_row(
                    "SELECT run_key, raw FROM capture_runs \
                      WHERE raw IS NOT NULL AND media_count > 0 ORDER BY id DESC LIMIT 1",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?)
        })
        .unwrap();
    let Some((run_key, raw)) = picked else {
        println!("副本里没有带产物的记录 —— 跳过");
        return;
    };
    let entry: serde_json::Value = serde_json::from_str(&raw).expect("raw 是 JSON");
    let config = comfyhub_server_rs::capture::capture_config(&state).expect("读配置");
    let request = comfyhub_server_rs::capture_ingest::build_ingest_request(&run_key, &entry, &config, false);
    println!(
        "重放 {run_key}：产物条目 {} 个，输出目录 {:?}，下载兜底 {}",
        request.outputs.len(),
        config.output_dir,
        config.download_fallback
    );

    // ⚠ 先把这条记录与它建的提示词从**副本**里清掉，否则 claim 会被幂等键挡住
    //    （这正是"同一次生成只建一条提示词"的实证）
    let claim_first = comfyhub_server_rs::capture_ingest::begin_run(&state, &run_key, "ComfyUI", Some(&entry), false)
        .expect("claim");
    println!("先抢一次（副本里已有 success 行）→ claimed={}", claim_first.claimed);
    assert!(!claim_first.claimed, "success 是终态，不该被重抢");

    state
        .write(|tx| {
            let prompt_id: Option<i64> = tx
                .query_row("SELECT prompt_id FROM capture_runs WHERE run_key = ?1", [&run_key], |r| r.get(0))
                .ok()
                .flatten();
            if let Some(id) = prompt_id {
                tx.execute("DELETE FROM media_assets WHERE prompt_id = ?1", [id])?;
                tx.execute("DELETE FROM prompts WHERE id = ?1", [id])?;
            }
            tx.execute("DELETE FROM capture_runs WHERE run_key = ?1", [&run_key])?;
            Ok(())
        })
        .unwrap();

    let claim = comfyhub_server_rs::capture_ingest::begin_run(&state, &run_key, "ComfyUI", Some(&entry), false)
        .expect("重新抢");
    assert!(claim.claimed, "清掉之后必须能重新抢");
    let outcome = comfyhub_server_rs::capture_ingest::ingest_claimed(&state, &request, &config).await;
    println!("结果：{}", outcome.to_json());
    // 产物文件可能早被用户删了 —— 那是**如实**的 failed，不是错误；但提示词一定要建出来
    assert!(outcome.created || outcome.message.is_some(), "要么建了提示词、要么如实说为什么没建");
    if let Some(prompt_id) = outcome.prompt_id {
        let (title, positive, seed): (String, String, Option<i64>) = state
            .with(|conn| {
                Ok(conn.query_row(
                    "SELECT title, positive_prompt, seed FROM prompts WHERE id = ?1",
                    [prompt_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?)
            })
            .unwrap();
        println!("提示词 {prompt_id}：title={title:?} seed={seed:?} positive[:60]={:?}", &positive.chars().take(60).collect::<String>());
        assert!(!title.trim().is_empty());
    }
}

/// **读真机上那些工作流文件**（第 44 步）：API 格式的应当零转换直接入库；
/// 界面格式的要问 ComfyUI 的 `/object_info`，此刻它没在跑 → **如实**报 `COMFY_UNREACHABLE`。
///
/// 这条抽查的价值在"**格式识别**"这一半：真实文件里两种格式混着（用户目录里既有
/// ComfyUI 保存的界面格式，也有「导出（API）」的节点图），认错了会走到完全不同的分支。
#[tokio::test]
#[ignore = "真库副本抽查：需要 COMFYHUB_SPOT_DB / COMFYHUB_SPOT_STORAGE"]
async fn real_db_spot_check_load_workflow_files() {
    let Some((state, root)) = spot_state() else { return };
    let policy = comfyhub_server_rs::policy::ToolPolicy::load(&state).expect("读策略");
    let config = comfyhub_server_rs::capture::capture_config(&state).expect("读配置");
    let dirs = comfyhub_server_rs::comfy_dirs::workflow_dirs(&root, config.output_dir.as_deref());
    let files = comfyhub_server_rs::comfy_dirs::scan(&dirs, None, 5);
    println!("真机上扫到 {} 个候选（列前 {} 个）", files.len(), 5.min(files.len()));
    let mut api_loaded = 0;
    let mut ui_refused = 0;
    for file in &files {
        // 路径准入：真实 ComfyUI 目录应当被自动放行（用户 bug ④）
        let resolved = match policy.resolve_read(&file.path) {
            Ok(path) => path,
            Err(e) => {
                println!("  × {} 被路径准入拒了：{}", file.name, e.message);
                continue;
            }
        };
        match comfyhub_server_rs::workflow_load::load_from_file(
            &state,
            &resolved,
            None,
            false,
            false,
            true,
        )
        .await
        {
            Ok(json) => {
                api_loaded += 1;
                println!(
                    "  ✓ {}（{}）→ promptId={} runnable={} 节点 {}",
                    file.name, file.format, json["promptId"], json["runnable"], json["nodeCount"]
                );
            }
            Err(failure) => {
                // 界面格式 + ComfyUI 没在跑 → 必须正好是这个码（别的码说明认错了格式）
                assert_eq!(
                    failure.code, "COMFY_UNREACHABLE",
                    "{} 的失败码不对（格式识别错了？）：{}",
                    file.name, failure.message
                );
                ui_refused += 1;
                println!("  · {}（{}）→ 如实拒绝：{}", file.name, file.format, failure.message);
            }
        }
    }
    // 至少要有文件被正确判成两种格式之一（一个文件都没有说明抽查没生效）
    assert!(api_loaded + ui_refused > 0, "一个文件都没读到 —— 抽查没生效");
    println!("API 直接入库 {api_loaded} 个；界面格式按「ComfyUI 没在跑」如实拒绝 {ui_refused} 个");
}

/// `comfy_list_workflows` + `comfy_get_run` + 图解析对账：真库 + 真 ComfyUI 目录上抽查。
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

// ===========================================================================
//  danbooru：**真站**抽查（要出网，默认不跑）
//
//  与 Kotlin 侧同一个口子（`COMFYHUB_LIVE_DANBOORU=1`）。它证明的不是"代码能编译"，
//  而是这条链路在本机**真的通**：短 UA 过得去、公网走代理、四步查询拿全。
//  假服务（tests/danbooru_test.rs）覆盖不了的就是这两条 —— 代理与 Cloudflare 的形状。
// ===========================================================================

#[tokio::test]
#[ignore = "真站抽查（要出网）：COMFYHUB_LIVE_DANBOORU=1"]
async fn live_danbooru_lookup() {
    if std::env::var("COMFYHUB_LIVE_DANBOORU").ok().as_deref() != Some("1") {
        eprintln!("跳过：没有设 COMFYHUB_LIVE_DANBOORU=1");
        return;
    }
    // danbooru 这条工具不碰库，所以用内存库造一个最小的 state 就行
    let conn = comfyhub_server_rs::db::open_memory().expect("内存库");
    let media = std::env::temp_dir().join("comfyhub-live-danbooru").join("media");
    std::fs::create_dir_all(&media).ok();
    let state = AppState::new(conn, ":memory:", media);

    let outcome = call(&state, "danbooru_lookup", serde_json::json!({ "query": "shinku (nte)" })).await;
    println!("{}", serde_json::to_string_pretty(&outcome).unwrap_or_default());
    assert_eq!(outcome["character"]["tag"], "shinku_(nte)", "角色 tag 要真的解析出来：{outcome}");
    assert!(
        outcome["character"]["relatedTags"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "同现 tag 是规范 tag 栈的来源，不该是空的：{outcome}"
    );
    assert!(outcome["character"]["otherNames"].is_array(), "{outcome}");
    assert!(outcome["verifiedAt"].as_str().unwrap_or("").ends_with('Z'), "{outcome}");
}

// ===========================================================================
//  comfy_env_report：真机环境对账（要一台真装了 ComfyUI 的机器）
//
//  它证明的是**盘扫那一半真的扫到了东西**：夹具里的临时目录永远长不出
//  `models\checkpoints\*.safetensors` 那种真实布局，也长不出 ComfyUI Desktop
//  把程序与共享数据分在两处的那种形状（那正是这个工具要回答的问题）。
// ===========================================================================

#[tokio::test]
#[ignore = "真库 + 真 ComfyUI 目录：COMFYHUB_SPOT_DB 指一份库副本"]
async fn comfy_env_report_reads_the_real_machine() {
    let Some((state, root)) = spot_state() else { return };
    let payload = call(&state, "comfy_env_report", serde_json::json!({ "limit": 5 })).await;
    println!("{}", serde_json::to_string_pretty(&payload).unwrap_or_default());

    // 形状（这些键是模型据以判断"要不要让用户重启"的全部依据）
    for key in ["instanceLoadable", "onDisk", "invisibleToInstance", "merged", "inventorySource"] {
        assert!(payload.get(key).is_some(), "少了 {key}：{payload}");
    }
    assert!(
        ["object_info", "scan", "none"].contains(&payload["inventorySource"].as_str().unwrap_or("")),
        "{payload}"
    );
    assert!(payload["models"].is_array(), "{payload}");

    // 真机上必须**扫到东西**（这台机器的 ComfyUI 根是探得到的；扫不到说明三份目录来源断了）
    let on_disk = payload["onDisk"].as_i64().unwrap_or(0);
    println!("project_root = {}", root.display());
    println!("盘上扫到 {on_disk} 个模型，实例能加载 {} 个", payload["instanceLoadable"]);
    assert!(on_disk > 0, "真机上扫不到任何模型 —— 三份目录来源或后缀表断了：{payload}");

    // 带过滤再问一次：只列 checkpoints，且条数不许超过 limit
    let only = call(&state, "comfy_env_report", serde_json::json!({ "folder": "checkpoints", "limit": 3 })).await;
    assert!(only["models"].as_array().unwrap().len() <= 3, "{only}");
    assert!(
        only["models"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["folder"] == "checkpoints"),
        "{only}"
    );
    assert_eq!(only["instanceLoadable"], payload["instanceLoadable"], "两次问的是同一份清单");
}
