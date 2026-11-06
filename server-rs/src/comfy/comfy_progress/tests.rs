//! `comfy_progress` 的单元测试（主体代码在 [`comfy_progress`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn tracker() -> ComfyProgress {
    ComfyProgress::new()
}

// ---- 真帧（2026-09-27 在本机 ComfyUI 上实测抓到的原文）----------------

const REAL_STATUS_HANDSHAKE: &str = r#"{"type": "status", "data": {"status": {"exec_info": {"queue_remaining": 0}}, "sid": "probe-dsh-14ebfb53"}}"#;
const REAL_STATUS_QUEUED: &str =
    r#"{"type": "status", "data": {"status": {"exec_info": {"queue_remaining": 1}}}}"#;
const REAL_EXECUTION_START: &str = r#"{"type": "execution_start", "data": {"prompt_id": "3cf2b8f7-babe-4a75-9b1c-4d120af3bc19", "timestamp": 1790510532242}}"#;
const REAL_EXECUTION_CACHED: &str = r#"{"type": "execution_cached", "data": {"nodes": ["1", "2"], "prompt_id": "3cf2b8f7-babe-4a75-9b1c-4d120af3bc19", "timestamp": 1790510532243}}"#;
const REAL_PROGRESS_STATE: &str = r#"{"type": "progress_state", "data": {"prompt_id": "bd810836-3331-4b52-832f-1514d6c3f5d4", "nodes": {"1": {"value": 1.0, "max": 1.0, "state": "finished", "node_id": "1", "prompt_id": "bd810836-3331-4b52-832f-1514d6c3f5d4", "display_node_id": "1", "parent_node_id": null, "real_node_id": "1"}, "2": {"value": 1.0, "max": 1.0, "state": "finished", "node_id": "2", "prompt_id": "bd810836-3331-4b52-832f-1514d6c3f5d4", "display_node_id": "2", "parent_node_id": null, "real_node_id": "2"}, "3": {"value": 0.0, "max": 1.0, "state": "running", "node_id": "3", "prompt_id": "bd810836-3331-4b52-832f-1514d6c3f5d4", "display_node_id": "3", "parent_node_id": null, "real_node_id": "3"}}}}"#;
const REAL_EXECUTING: &str = r#"{"type": "executing", "data": {"node": "5", "display_node": "5", "prompt_id": "3cf2b8f7-babe-4a75-9b1c-4d120af3bc19"}}"#;
const REAL_EXECUTING_DONE: &str = r#"{"type": "executing", "data": {"node": null, "prompt_id": "bd810836-3331-4b52-832f-1514d6c3f5d4"}}"#;
const REAL_EXECUTED: &str = r#"{"type": "executed", "data": {"node": "5", "display_node": "5", "output": {"images": [{"filename": "ComfyUI_temp_nffhb_00001_.png", "subfolder": "", "type": "temp"}]}, "prompt_id": "bd810836-3331-4b52-832f-1514d6c3f5d4"}}"#;
const REAL_EXECUTION_SUCCESS: &str = r#"{"type": "execution_success", "data": {"prompt_id": "bd810836-3331-4b52-832f-1514d6c3f5d4", "timestamp": 1790510888972}}"#;
const REAL_EXECUTION_ERROR: &str = r#"{"type": "execution_error", "data": {"prompt_id": "3cf2b8f7-babe-4a75-9b1c-4d120af3bc19", "node_id": "5", "node_type": "CLIPTextEncode", "executed": [], "exception_message": "ERROR: clip input is invalid: None\n\nIf the clip is from a checkpoint loader node your checkpoint does not contain a valid clip or text encoder model.\n", "exception_type": "RuntimeError", "traceback": ["  File \"execution.py\", line 547, in execute\n"], "current_inputs": {"clip": [null], "text": [""]}, "current_outputs": ["2", "1", "5", "7", "4", "6", "3"], "timestamp": 1790510532249}}"#;

/// ⚠ **手写**帧（不是实测）：本机这个 ComfyUI 版本的常见节点没走
/// `hijack_progress` 的 `ProgressBar` 钩子，所以经典 `progress` 帧没抓到活帧。
/// 形状抄自 `main.py:432` —— `progress = {"value": value, "max": total,
/// "prompt_id": prompt_id, "node": node_id}`。
const HANDWRITTEN_PROGRESS: &str = r#"{"type": "progress", "data": {"value": 12, "max": 30, "prompt_id": "run-progress-1", "node": "3"}}"#;

#[test]
fn 真帧_status_解析出队列剩余数() {
    assert_eq!(
        parse_frame(REAL_STATUS_HANDSHAKE),
        Some(ProgressEvent::Status {
            queue_remaining: Some(0)
        })
    );
    assert_eq!(
        parse_frame(REAL_STATUS_QUEUED),
        Some(ProgressEvent::Status {
            queue_remaining: Some(1)
        })
    );
}

#[test]
fn 真帧_各个生命周期事件都能解析() {
    assert_eq!(
        parse_frame(REAL_EXECUTION_START),
        Some(ProgressEvent::ExecutionStart {
            prompt_id: "3cf2b8f7-babe-4a75-9b1c-4d120af3bc19".into()
        })
    );
    assert_eq!(
        parse_frame(REAL_EXECUTION_CACHED),
        Some(ProgressEvent::ExecutionCached {
            prompt_id: "3cf2b8f7-babe-4a75-9b1c-4d120af3bc19".into()
        })
    );
    assert_eq!(
        parse_frame(REAL_EXECUTING),
        Some(ProgressEvent::Executing {
            prompt_id: "3cf2b8f7-babe-4a75-9b1c-4d120af3bc19".into(),
            node_id: Some("5".into())
        })
    );
    // `executing` 的 node 可以是 null：那是"没有节点在执行了"
    assert_eq!(
        parse_frame(REAL_EXECUTING_DONE),
        Some(ProgressEvent::Executing {
            prompt_id: "bd810836-3331-4b52-832f-1514d6c3f5d4".into(),
            node_id: None
        })
    );
    assert_eq!(
        parse_frame(REAL_EXECUTED),
        Some(ProgressEvent::Executed {
            prompt_id: "bd810836-3331-4b52-832f-1514d6c3f5d4".into(),
            node_id: Some("5".into())
        })
    );
    assert_eq!(
        parse_frame(REAL_EXECUTION_SUCCESS),
        Some(ProgressEvent::ExecutionSuccess {
            prompt_id: "bd810836-3331-4b52-832f-1514d6c3f5d4".into()
        })
    );
}

#[test]
fn 真帧_progress_state_取正在跑的那个节点() {
    // 帧里有 1/2（finished）与 3（running）——**必须**取 running 那个
    assert_eq!(
        parse_frame(REAL_PROGRESS_STATE),
        Some(ProgressEvent::ProgressState {
            prompt_id: "bd810836-3331-4b52-832f-1514d6c3f5d4".into(),
            node_id: Some("3".into()),
            value: 0.0,
            max: 1.0
        })
    );
    // 一个 running 都没有（全是 finished）→ 这帧不带新进度，忽略
    let all_finished = r#"{"type":"progress_state","data":{"prompt_id":"p1","nodes":{"1":{"value":1.0,"max":1.0,"state":"finished"}}}}"#;
    assert_eq!(parse_frame(all_finished), None);
}

#[test]
fn 真帧_execution_error_带节点与原始报错() {
    let event = parse_frame(REAL_EXECUTION_ERROR).expect("要认出来");
    match event {
        ProgressEvent::ExecutionError {
            prompt_id,
            node_id,
            message,
        } => {
            assert_eq!(prompt_id, "3cf2b8f7-babe-4a75-9b1c-4d120af3bc19");
            assert_eq!(node_id.as_deref(), Some("5"));
            assert!(message.unwrap().contains("clip input is invalid"));
        }
        other => panic!("解析成了 {other:?}"),
    }
}

#[test]
fn 手写帧_经典_progress_也能解析() {
    assert_eq!(
        parse_frame(HANDWRITTEN_PROGRESS),
        Some(ProgressEvent::Progress {
            prompt_id: "run-progress-1".into(),
            node_id: Some("3".into()),
            value: 12.0,
            max: 30.0
        })
    );
    // 喂进跟踪器 → 40%
    let tracker = tracker();
    tracker.frame(HANDWRITTEN_PROGRESS);
    let progress = tracker.snapshot("run-progress-1").unwrap();
    assert_eq!(progress.percent, Some(40), "12/30 = 40%");
    assert_eq!(progress.value, Some(12));
    assert_eq!(progress.max, Some(30));
    assert_eq!(progress.state, "running");
}

#[test]
fn 未知帧与坏帧一律不许_panic() {
    for text in [
        "",
        "not json",
        "null",
        "[]",
        "{}",
        r#"{"type": null}"#,
        // 自定义节点会发一堆我们不认识的东西
        r#"{"type": "logs", "data": {"entries": []}}"#,
        r#"{"type": "crystools.monitor", "data": {}}"#,
        r#"{"type": "progress", "data": {}}"#, // 缺 value/max/promptId
        r#"{"type": "progress", "data": {"value": 1, "max": 0}}"#, // 缺 prompt_id
        r#"{"type": "executing", "data": {}}"#, // 缺 prompt_id
        r#"{"type": "progress_state", "data": {"prompt_id": "p"}}"#, // 缺 nodes
        r#"{"type": "status", "data": null}"#,
        r#"{"type": "execution_error", "data": {"prompt_id": "p", "exception_message": null}}"#,
    ] {
        let parsed = parse_frame(text);
        assert!(
            parsed.is_none()
                || matches!(
                    parsed,
                    Some(ProgressEvent::ExecutionError { .. }) | Some(ProgressEvent::Status { .. })
                ),
            "这条不该被解析成进度事件：{text}（得到 {parsed:?}）"
        );
    }
    // 喂进跟踪器也不许炸，而且不许凭空造出一条记录
    let tracker = tracker();
    for text in ["", "not json", r#"{"type": "logs"}"#, r#"{"type": "nope"}"#] {
        tracker.frame(text);
    }
    assert!(tracker.current().is_none());
    assert!(tracker.snapshot("任何").is_none());
}

#[test]
fn 整条真帧序列跑一遍_状态与百分比都对() {
    let tracker = tracker();
    // 提交时登记节点名（AI 提交时手里就有 API 图）
    tracker.register_graph(
        "bd810836-3331-4b52-832f-1514d6c3f5d4",
        &json!({
            "1": { "class_type": "VAELoader", "_meta": { "title": "加载 VAE" } },
            "3": { "class_type": "VAEEncodeTiled" },
            "5": { "class_type": "PreviewImage" },
        }),
    );
    tracker.frame(REAL_STATUS_HANDSHAKE);
    tracker.frame(REAL_EXECUTION_START);
    tracker.frame(REAL_EXECUTION_CACHED);
    tracker.frame(REAL_PROGRESS_STATE);
    tracker.frame(REAL_EXECUTING);
    tracker.frame(REAL_EXECUTED);
    tracker.frame(REAL_EXECUTION_SUCCESS);

    let progress = tracker
        .snapshot("bd810836-3331-4b52-832f-1514d6c3f5d4")
        .unwrap();
    assert_eq!(progress.state, "success", "终态帧说了成功");
    assert_eq!(progress.value, Some(0), "0/1 也是 ComfyUI 自己报的");
    assert_eq!(progress.max, Some(1));
    assert_eq!(progress.percent, Some(0), "不许在成功时把 percent 抬到 100");
    assert!(progress.node_id.is_none(), "跑完了就不该还挂着某个节点");
    assert_eq!(progress.error, None);
    // 队列剩余数记下来了
    assert_eq!(tracker.queue_remaining(), Some(0));
}

#[test]
fn 可读名来自提交时登记的图_没有映射时只给节点_id() {
    let tracker = tracker();
    tracker.register_graph(
        "p1",
        &json!({
            "1": { "class_type": "VAELoader", "_meta": { "title": "加载 VAE" } },
            "3": { "class_type": "KSampler" },
            "9": { "inputs": {} },   // 连 class_type 都没有 → 不登记
        }),
    );
    tracker.apply(&ProgressEvent::ExecutionStart {
        prompt_id: "p1".into(),
    });
    // ① 有 `_meta.title` → 用人起的标题
    tracker.apply(&ProgressEvent::Executing {
        prompt_id: "p1".into(),
        node_id: Some("1".into()),
    });
    assert_eq!(
        tracker.snapshot("p1").unwrap().node_name.as_deref(),
        Some("加载 VAE")
    );
    // ② 没有 title → 回落 class_type
    tracker.apply(&ProgressEvent::Executing {
        prompt_id: "p1".into(),
        node_id: Some("3".into()),
    });
    assert_eq!(
        tracker.snapshot("p1").unwrap().node_name.as_deref(),
        Some("KSampler")
    );
    // ③ 没登记过 → **只给 id**，绝不编名字
    tracker.apply(&ProgressEvent::Executing {
        prompt_id: "p1".into(),
        node_id: Some("9".into()),
    });
    let progress = tracker.snapshot("p1").unwrap();
    assert_eq!(progress.node_id.as_deref(), Some("9"));
    assert_eq!(progress.node_name, None, "没有映射时不许编一个名字");
    assert!(
        progress.human().contains("正在执行节点 9"),
        "{}",
        progress.human()
    );

    // ④ 整个 prompt 都没登记过（不是我们提交的）→ 同样只给 id
    tracker.apply(&ProgressEvent::Executing {
        prompt_id: "别人提交的".into(),
        node_id: Some("42".into()),
    });
    let progress = tracker.snapshot("别人提交的").unwrap();
    assert_eq!(progress.node_name, None);
    assert_eq!(progress.node_id.as_deref(), Some("42"));
}

#[test]
fn 同一个_prompt_的_percent_单调不倒退() {
    let tracker = tracker();
    let send = |value: f64| {
        tracker.apply(&ProgressEvent::Progress {
            prompt_id: "p1".into(),
            node_id: Some("3".into()),
            value,
            max: 30.0,
        });
    };
    send(12.0);
    assert_eq!(tracker.snapshot("p1").unwrap().percent, Some(40));
    send(20.0);
    assert_eq!(tracker.snapshot("p1").unwrap().percent, Some(67));
    // ComfyUI 补发一条更早的帧 → 界面**不许**倒着走
    send(3.0);
    assert_eq!(
        tracker.snapshot("p1").unwrap().percent,
        Some(67),
        "高水位只升不降"
    );
    send(30.0);
    assert_eq!(tracker.snapshot("p1").unwrap().percent, Some(100));
}

#[test]
fn 拿不到_value_max_时如实说进度未知() {
    let tracker = tracker();
    // `executing` 只说"轮到哪个节点了"，**没有任何 value/max** → percent 必须是 None
    tracker.apply(&ProgressEvent::Executing {
        prompt_id: "p1".into(),
        node_id: Some("3".into()),
    });
    let progress = tracker.snapshot("p1").unwrap();
    assert_eq!(
        progress.percent, None,
        "没有 progress 帧就是进度未知，不许拿别的凑"
    );
    assert_eq!(progress.value, None);
    assert_eq!(progress.max, None);
    assert!(
        progress.human().contains("进度未知"),
        "{}",
        progress.human()
    );
    // 序列化之后 `percent` / `value` / `max` **整键不出现**（可空键省略规则）
    let json = progress.to_json();
    assert!(json.get("percent").is_none());
    assert!(json.get("value").is_none());
    assert!(json.get("max").is_none());
    assert_eq!(json["state"], "running");
    // `max = 0` 这种脏数据也算未知
    assert_eq!(percent_of(1.0, 0.0), None);
}

#[test]
fn 断线重连后状态不丢() {
    let tracker = tracker();
    // 连上 → 收到几帧 → 断了（`note_error` 就是 `pump` 失败时做的事）
    tracker.apply(&ProgressEvent::ExecutionStart {
        prompt_id: "p1".into(),
    });
    tracker.apply(&ProgressEvent::Progress {
        prompt_id: "p1".into(),
        node_id: Some("3".into()),
        value: 9.0,
        max: 30.0,
    });
    tracker.connected.store(true, Ordering::SeqCst);
    assert!(tracker.is_connected());
    assert_eq!(tracker.snapshot("p1").unwrap().percent, Some(30));

    tracker.note_error("进度 WebSocket 读失败：connection reset");
    assert!(!tracker.is_connected(), "断了就要如实说断了");
    assert!(tracker.last_error().unwrap().contains("connection reset"));

    // ★ 重连**不清空**任何进度：断线期间界面仍要显示最后一次知道的事实
    let after = tracker.snapshot("p1").unwrap();
    assert_eq!(after.percent, Some(30), "重连后百分比不许归零");
    assert_eq!(after.value, Some(9));
    assert_eq!(after.state, "running");
    assert_eq!(after.node_id.as_deref(), Some("3"));
    assert_eq!(tracker.current().unwrap().prompt_id, "p1");

    // 断线期间 ComfyUI 把这一轮跑完了 → 重连后补到终态帧，进度还能对上
    tracker.frame(
        REAL_EXECUTION_SUCCESS
            .replace("bd810836-3331-4b52-832f-1514d6c3f5d4", "p1")
            .as_str(),
    );
    assert_eq!(tracker.snapshot("p1").unwrap().state, "success");
    assert!(tracker.current().is_none(), "终态之后就没有'正在跑的'了");
}

#[test]
fn 报错与取消都记成人话状态() {
    let tracker = tracker();
    tracker.register_graph("p1", &json!({ "5": { "class_type": "CLIPTextEncode" } }));
    tracker.apply(&ProgressEvent::ExecutionStart {
        prompt_id: "p1".into(),
    });
    tracker.apply(&ProgressEvent::ExecutionError {
        prompt_id: "p1".into(),
        node_id: Some("5".into()),
        message: Some("ERROR: clip input is invalid".into()),
    });
    let progress = tracker.snapshot("p1").unwrap();
    assert_eq!(progress.state, "error");
    assert_eq!(
        progress.node_name.as_deref(),
        Some("CLIPTextEncode"),
        "报错也要说清是哪个节点"
    );
    assert!(progress.error.unwrap().contains("clip input is invalid"));
    assert!(tracker.current().is_none());

    tracker.apply(&ProgressEvent::ExecutionInterrupted {
        prompt_id: "p2".into(),
    });
    assert_eq!(tracker.snapshot("p2").unwrap().state, "cancelled");
}

#[test]
fn current_只跟着正在跑的那一个走() {
    let tracker = tracker();
    assert!(tracker.current().is_none(), "什么都没跑");
    tracker.apply(&ProgressEvent::ExecutionStart {
        prompt_id: "p1".into(),
    });
    assert_eq!(tracker.current().unwrap().prompt_id, "p1");
    // 队列里排上第二条：它还是 queued，**不该**抢走 current
    tracker.apply(&ProgressEvent::Executing {
        prompt_id: "p2".into(),
        node_id: Some("1".into()),
    });
    assert_eq!(
        tracker.current().unwrap().prompt_id,
        "p2",
        "真的开始执行了才轮到它"
    );
    tracker.apply(&ProgressEvent::ExecutionSuccess {
        prompt_id: "p1".into(),
    });
    assert_eq!(
        tracker.current().unwrap().prompt_id,
        "p2",
        "p1 结束了不该把 p2 也清掉"
    );
    tracker.apply(&ProgressEvent::ExecutionError {
        prompt_id: "p2".into(),
        node_id: None,
        message: None,
    });
    assert!(tracker.current().is_none());
}

#[test]
fn 给提交记录补进度_没有进度时原样不动() {
    let tracker = tracker();
    let submission = json!({ "promptId": "p1", "status": "running" });
    assert_eq!(
        tracker.decorate(&submission),
        submission,
        "没有进度就原样返回（不塞一个空对象）"
    );
    tracker.apply(&ProgressEvent::ProgressState {
        prompt_id: "p1".into(),
        node_id: Some("3".into()),
        value: 6.0,
        max: 30.0,
    });
    let decorated = tracker.decorate(&submission);
    assert_eq!(decorated["progress"]["percent"], 20);
    assert_eq!(decorated["progress"]["nodeId"], "3");
    assert_eq!(decorated["status"], "running", "原有字段一个都不许动");
    // 没有 promptId 的脏记录也不许炸
    assert_eq!(tracker.decorate(&json!({})), json!({}));
}

#[test]
fn ws_url_只连明文且带同一个_client_id() {
    assert_eq!(
        ws_url("http://127.0.0.1:8188", "cid"),
        Some("ws://127.0.0.1:8188/ws?clientId=cid".to_string())
    );
    // 尾斜杠 / 空白都要归一化（用户设置页里手打的地址常有尾斜杠）
    assert_eq!(
        ws_url("  http://127.0.0.1:8188/  ", "cid"),
        Some("ws://127.0.0.1:8188/ws?clientId=cid".to_string())
    );
    assert_eq!(ws_url("", "cid"), None);
    // https → wss 只是**如实透传**（我们没编 TLS 后端，连不上就如实记错误）
    assert_eq!(
        ws_url("https://example.com", "cid"),
        Some("wss://example.com/ws?clientId=cid".to_string())
    );
}

#[test]
fn 提交时带的就是_ws_同一个_client_id() {
    // 两者不一致 = "WS 连上了但一帧都收不到"（ComfyUI 只发给 extra_data.client_id 那个 socket）
    let tracker = ComfyProgress::new();
    let id = tracker.client_id();
    assert!(id.starts_with("comfyhub-"));
    assert!(ws_url("http://127.0.0.1:8188", id).unwrap().ends_with(id));
    // 两个实例的 id 不同（不然两个后端会在 ComfyUI 的 socket 表里互相顶掉）
    assert_ne!(tracker.client_id(), ComfyProgress::new().client_id());
}

#[test]
fn 进度表不会无限涨() {
    let tracker = tracker();
    for index in 0..(MAX_TRACKED_PROMPTS + 5) {
        tracker.apply(&ProgressEvent::ExecutionSuccess {
            prompt_id: format!("p{index:03}"),
        });
    }
    let inner = tracker.lock();
    assert_eq!(inner.prompts.len(), MAX_TRACKED_PROMPTS);
    assert!(inner.node_names.len() <= MAX_TRACKED_PROMPTS);
    assert!(inner.percent_high_water.len() <= MAX_TRACKED_PROMPTS);
}

// -----------------------------------------------------------------------
//  WS 客户端本身：起一个**本机假服务端**（真 WebSocket 握手 + 真帧），
//  验"连上 → 收帧 → 断线"这一整条。**不碰真 ComfyUI**。
// -----------------------------------------------------------------------

/// 跑一次 `pump`，返回服务端看到的请求路径。
// `accept_hdr_async` 的 Err 是 tungstenite 的 `ErrorResponse`（很大）；这是测试里的
// 假服务端，不值得为它把错误装箱。
#[allow(clippy::result_large_err)]
async fn pump_against(frames: Vec<&'static str>, tracker: Arc<ComfyProgress>) -> String {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (path_tx, path_rx) = tokio::sync::oneshot::channel::<String>();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        // `accept_hdr_async` 让我们看到客户端**真的**请求了哪条路径 ——
        // `?clientId=` 丢了的话 ComfyUI 会给一个随机 sid，进度一帧都收不到
        let callback =
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
             response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                let _ = path_tx.send(request.uri().to_string());
                Ok(response)
            };
        let mut socket = tokio_tungstenite::accept_hdr_async(stream, callback)
            .await
            .unwrap();
        for frame in frames {
            socket.send(Message::text(frame)).await.unwrap();
        }
        // 留一点时间让客户端读完，然后**正常关闭**（模拟 ComfyUI 重启）
        tokio::time::sleep(Duration::from_millis(200)).await;
        socket.close(None).await.ok();
        // 把 socket 留住到 drop（客户端先读到 Close 帧）
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let url = format!("ws://{addr}/ws?clientId={}", tracker.client_id());
    pump(&tracker, &url)
        .await
        .expect("假服务端一直好好的，pump 该回 Ok");
    server.await.unwrap();
    path_rx.await.unwrap()
}

#[tokio::test]
async fn ws_客户端把真帧喂进跟踪器且带上_client_id() {
    let tracker = Arc::new(ComfyProgress::new());
    let path = pump_against(
        vec![
            REAL_STATUS_HANDSHAKE,
            REAL_EXECUTION_START,
            REAL_EXECUTION_CACHED,
            REAL_PROGRESS_STATE,
            REAL_EXECUTING,
            REAL_EXECUTED,
            REAL_EXECUTION_SUCCESS,
        ],
        tracker.clone(),
    )
    .await;
    assert!(
        path.starts_with("/ws?clientId=comfyhub-"),
        "握手请求必须是 /ws?clientId=<我们的 id>，实际是 {path}"
    );
    let progress = tracker
        .snapshot("bd810836-3331-4b52-832f-1514d6c3f5d4")
        .unwrap();
    assert_eq!(progress.state, "success");
    assert_eq!(progress.percent, Some(0), "0/1：ComfyUI 自己报的，没编");
    // 服务端正常关闭 → 如实说"断了"（`spawn` 据此退避重连），进度**不丢**
    assert!(!tracker.is_connected());
    assert!(tracker.last_error().is_none(), "正常关闭不算错误");
}

#[tokio::test]
async fn ws_客户端连不上时把人话错误交出来() {
    let tracker = Arc::new(ComfyProgress::new());
    // 127.0.0.1:1（没人听）+ 我们的 client_id：URL 形状与真跑一致
    let url = format!("ws://127.0.0.1:1/ws?clientId={}", tracker.client_id());
    let error = pump(&tracker, &url).await.expect_err("连不上必须是 Err");
    assert!(error.contains("连不上 ComfyUI 的进度 WebSocket"), "{error}");
    assert!(!tracker.is_connected());
}
