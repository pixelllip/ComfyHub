//! `workflow_submit` 的单元测试（主体代码在 [`workflow_submit`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn graph() -> Value {
    json!({
        "3": {
            "class_type": "KSampler",
            "inputs": { "seed": 42, "steps": 20, "cfg": 7.5, "denoise": 1.0, "model": ["4", 0] }
        },
        "4": {
            "class_type": "CheckpointLoaderSimple",
            "inputs": { "ckpt_name": "krea2.safetensors" }
        },
        "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "old prompt", "clip": ["4", 1] } }
    })
}

#[test]
fn 覆盖参数按原值类型转换() {
    let mut overrides = Map::new();
    overrides.insert("3.steps".into(), json!("30")); // 原是整数 → 转成整数
    overrides.insert("6.text".into(), json!("new prompt")); // 原是文本
    overrides.insert("3.cfg".into(), json!(1.0)); // 数字原样
    let (patched, applied) = WorkflowEdit::apply_overrides(&graph(), &overrides).unwrap();
    assert_eq!(patched["3"]["inputs"]["steps"], json!(30));
    assert_eq!(patched["6"]["inputs"]["text"], json!("new prompt"));
    assert_eq!(patched["3"]["inputs"]["cfg"], json!(1.0));
    assert_eq!(applied.len(), 3);
    assert!(applied.contains(&"3.steps ← 30".to_string()));
    // 原图不许被改（纯函数）
    assert_eq!(graph()["3"]["inputs"]["steps"], json!(20));
}

#[test]
fn 覆盖参数的四种拒绝都是人话() {
    let mut bad_path = Map::new();
    bad_path.insert("3".into(), json!("x"));
    assert!(WorkflowEdit::apply_overrides(&graph(), &bad_path).unwrap_err().contains("节点id.输入名"));

    let mut no_node = Map::new();
    no_node.insert("99.steps".into(), json!(1));
    assert!(WorkflowEdit::apply_overrides(&graph(), &no_node).unwrap_err().contains("没有节点 99"));

    let mut no_input = Map::new();
    no_input.insert("3.nope".into(), json!(1));
    let message = WorkflowEdit::apply_overrides(&graph(), &no_input).unwrap_err();
    assert!(message.contains("没有名为「nope」的输入"), "{message}");
    assert!(message.contains("seed"), "要列出已有的输入名：{message}");

    let mut wrong_type = Map::new();
    wrong_type.insert("3.steps".into(), json!("很多"));
    assert!(WorkflowEdit::apply_overrides(&graph(), &wrong_type)
        .unwrap_err()
        .contains("原来是整数"));

    let mut non_scalar = Map::new();
    non_scalar.insert("3.steps".into(), json!([1, 2]));
    assert!(WorkflowEdit::apply_overrides(&graph(), &non_scalar)
        .unwrap_err()
        .contains("只支持标量"));
}

#[test]
fn 补线允许新输入但要求上游节点存在() {
    let mut connections = Map::new();
    // VAELoader 那边缺的线：节点 5 是空的（转换时被摘过），这里补一个**新输入名**
    let mut with_hole = graph();
    with_hole["6"]["inputs"].as_object_mut().unwrap().remove("clip");
    connections.insert("6.clip".into(), json!(["4", 1]));
    let (patched, applied) = WorkflowEdit::apply_connections(&with_hole, &connections).unwrap();
    assert_eq!(patched["6"]["inputs"]["clip"], json!(["4", 1]));
    assert_eq!(applied, vec!["6.clip ← [4, 1]".to_string()]);

    let mut bad_source = Map::new();
    bad_source.insert("6.clip".into(), json!(["77", 0]));
    let message = WorkflowEdit::apply_connections(&graph(), &bad_source).unwrap_err();
    assert!(message.contains("节点 77 不在这张图里"), "{message}");

    let mut bad_shape = Map::new();
    bad_shape.insert("6.clip".into(), json!(["4"]));
    assert!(WorkflowEdit::apply_connections(&graph(), &bad_shape).unwrap_err().contains("两个元素"));

    let mut bad_slot = Map::new();
    bad_slot.insert("6.clip".into(), json!(["4", "第一个"]));
    assert!(WorkflowEdit::apply_connections(&graph(), &bad_slot).unwrap_err().contains("槽位要是整数"));

    // 数字 id 与字符串 id 必须都能用（真库里有 `"negative": [76, 0]` 这种）
    let mut numeric = Map::new();
    numeric.insert("6.clip".into(), json!([4, 1]));
    let (patched, _) = WorkflowEdit::apply_connections(&graph(), &numeric).unwrap();
    assert_eq!(patched["6"]["inputs"]["clip"], json!([4, 1]));
}

fn object_info() -> Value {
    json!({
        "KSampler": { "input": { "required": { "seed": ["INT", {}], "steps": ["INT", {}] } } },
        "CheckpointLoaderSimple": {
            "input": { "required": { "ckpt_name": [["krea2.safetensors", "anima.safetensors"], {}] } }
        },
        "CLIPTextEncode": { "input": { "required": { "text": ["STRING", {}] } } }
    })
}

// ---------------------------------------------------------------------------
//  「这份工作流认不认负面提示词」（2026-10-11）
// ---------------------------------------------------------------------------

/// 一份**有**负面输入的采样器（`negative` 是 optional 条件输入）。
fn info_with_negative() -> Map<String, Value> {
    serde_json::from_value(json!({
        "KSampler": {
            "input": {
                "required": { "seed": ["INT", {}], "positive": ["CONDITIONING", {}] },
                "optional": { "negative": ["CONDITIONING", {}] }
            }
        },
        // 认不出来源的类：`/object_info` 里没有 → 不参与判定（不猜）
        "CLIPTextEncode": { "input": { "required": { "text": ["STRING", {}] } } }
    }))
    .unwrap()
}

#[test]
fn 负面能力_接了线才算认() {
    // 采样器的 negative 接了线 → 认
    let wired = json!({
        "3": { "class_type": "KSampler",
               "inputs": { "seed": 1, "positive": ["6", 0], "negative": ["7", 0] } },
        "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "1girl" } },
        "7": { "class_type": "CLIPTextEncode", "inputs": { "text": "bad hands" } }
    });
    assert_eq!(
        negative_capability(&wired, Some(&info_with_negative())),
        Some(vec!["KSampler".to_string()]),
        "negative 接了线 → 这份工作流认负面"
    );
}

#[test]
fn 负面能力_声明了但没接线不算() {
    // 采样器声明了 negative，但图里**没接** —— 等于没用到（这正是 turbo 那种工作流的形状）
    let unwired = json!({
        "3": { "class_type": "KSampler", "inputs": { "seed": 1, "positive": ["6", 0] } },
        "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "1girl" } }
    });
    assert_eq!(
        negative_capability(&unwired, Some(&info_with_negative())),
        Some(Vec::new()),
        "查过了、确实没有负面输入 → 空数组（**不是** None）"
    );
}

#[test]
fn 负面能力_只有正面文本编码器不算认负面() {
    // 最常见的误判：图里**有** CLIPTextEncode 就以为"认负面"。
    // 那个其实是正面那一个 —— 判据必须落在"采样器的 negative 接没接线"上。
    let only_positive = json!({
        "3": { "class_type": "KSampler", "inputs": { "seed": 1, "positive": ["6", 0] } },
        "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "1girl" } }
    });
    assert_eq!(negative_capability(&only_positive, Some(&info_with_negative())), Some(Vec::new()));
}

#[test]
fn 负面能力_拿不到_object_info_就回_None_不猜() {
    let graph = json!({ "3": { "class_type": "KSampler", "inputs": { "negative": ["7", 0] } } });
    assert_eq!(
        negative_capability(&graph, None),
        None,
        "ComfyUI 没跑（拿不到 /object_info）时不许说'这份图不认负面'"
    );
}

#[test]
fn 预检_节点类拦_模型文件只警告() {    assert!(preflight_refusal(&graph(), &object_info()).is_none(), "都对就必须放行");

    // ① 实例不认识的节点类 → **拦**（`/object_info` 对节点类是权威的）
    let mut missing_node = graph();
    missing_node["12"] = json!({ "class_type": "Krea2CustomNode", "inputs": {} });
    let refusal = preflight_refusal(&missing_node, &object_info()).unwrap();
    assert!(refusal.contains("实例里没有这些节点类"), "{refusal}");
    assert!(refusal.contains("12: Krea2CustomNode"), "{refusal}");
    assert!(refusal.contains("不要") && refusal.contains("改用户的工作流"), "{refusal}");

    // ② 模型文件不在实例的候选里 → **只警告不拦**（见 `preflight_refusal` 的说明：
    //    盘扫还没搬，只按候选判会误拦；e2e-submit 第 4/5 幕就是活证）
    let mut wrong_model = graph();
    wrong_model["4"]["inputs"]["ckpt_name"] = json!("anima-base-v1.0.safetensors");
    assert!(
        preflight_refusal(&wrong_model, &object_info()).is_none(),
        "模型文件那一档现在是提示，不许拦"
    );
    let warnings = preflight_warnings(&wrong_model, &object_info());
    assert_eq!(warnings.len(), 3, "首行 + 一条明细 + 一条动作：{warnings:?}");
    let joined = warnings.join("\n");
    // ⚠ 这一档的措辞 2026-09-30 由用户自己精简过（去掉了"没有拦""重启 ComfyUI""别去下载"）。
    //   **"先让用户重启实例 / 修模型根、别去下载"那条纪律现在只由工具描述承担**
    //   （`desktop_tools` 里 `comfy_submit` / `comfy_env_report` 的描述 + 系统提示），
    //   所以这里只钉"这条警告本身必须说清的事"：**看不见几个、是哪一个、去哪儿看全**。
    assert!(joined.contains("看不到 1 个模型文件"), "要说清数量：{joined}");
    assert!(joined.contains("4.ckpt_name 要 anima-base-v1.0.safetensors"), "{joined}");
    assert!(joined.contains("comfy_env_report"), "要给下一步去哪儿看全：{joined}");
    // 都对的时候没有警告
    assert!(preflight_warnings(&graph(), &object_info()).is_empty());

    // ③ 非加载器节点的候选列表**不参与**判定（图片输入那种动态列表会误报）
    let mut with_image = graph();
    with_image["9"] = json!({ "class_type": "LoadImage", "inputs": { "image": "not-in-list.png" } });
    let mut info = object_info();
    info["LoadImage"] = json!({ "input": { "required": { "image": [["a.png"], {}] } } });
    assert!(preflight_refusal(&with_image, &info).is_none(), "LoadImage 不是 Loader，不判它");
    assert!(preflight_warnings(&with_image, &info).is_empty(), "也不该警告");
}

#[test]
fn 纯前端节点不走实例侧这一档_交给审核那一条说() {
    // 对 `Label (rgthree)` / `Anything Everywhere` 说"让用户装/启用节点包"是**错的方向**：
    // 这些类只存在于 ComfyUI 前端，服务端根本没有（装也装不上）。
    // 这一档必须让给 `workflow_audit` 里那条"服务端根本没有这些类"的结论。
    let mut frontend = graph();
    frontend["2"] = json!({ "class_type": "Label (rgthree)", "inputs": {} });
    frontend["5"] = json!({ "class_type": "Anything Everywhere", "inputs": {} });
    frontend["8"] = json!({ "class_type": "PreviewImage", "inputs": {} });
    assert!(
        preflight_refusal(&frontend, &object_info()).is_none(),
        "纯前端节点要被跳过（它们不在这一档的射程里）"
    );
    // 而审核那一档真的会拦它们（否则上面那个 continue 就成了漏网）
    let info = object_info();
    let report =
        workflow_audit::audit(frontend.as_object().unwrap(), info.as_object(), None);
    assert!(
        report.blocking_refusal().is_some(),
        "纯前端节点必须由审核拦下：{}",
        report.render()
    );
    // 判据本身（三张表/模式的合取）钉一下
    for cls in ["Label (rgthree)", "Anything Everywhere", "PreviewImage", "Display Any"] {
        assert!(workflow_audit::is_frontend_only(cls), "{cls}");
    }
    for cls in ["Krea2CustomNode", "KSampler", "CheckpointLoaderSimple"] {
        assert!(!workflow_audit::is_frontend_only(cls), "{cls} 不是纯前端类");
    }
}

#[test]
fn 提交记录的保留窗口与就地更新() {
    let submissions = Submissions::new();
    for i in 0..(MAX_TRACKED + 5) {
        let mut item = Submission {
            prompt_id: format!("run-{i:03}"),
            submitted_by: "ai".into(),
            title: None,
            status: "queued".into(),
            submitted_at: format!("2026-09-26T00:00:{i:02}Z"),
            finished_at: None,
            elapsed_ms: 0,
            media_ids: Vec::new(),
            captured_prompt_id: None,
            error: None,
            message: None,
        }
        .to_json();
        // submittedAt 只到秒，秒数超过 59 会不合法 —— 直接改成递增的分钟
        item["submittedAt"] = json!(format!("2026-09-26T00:{:02}:00Z", i));
        submissions.record(item);
    }
    let listed = submissions.list(MAX_TRACKED);
    assert_eq!(listed.len(), MAX_TRACKED, "只留最近 {MAX_TRACKED} 条");
    assert_eq!(listed[0]["promptId"], json!(format!("run-{:03}", MAX_TRACKED + 4)), "新的在前");

    // 同一个 promptId 就地更新，不新增一条
    let before = submissions.list(MAX_TRACKED).len();
    let mut updated = Submission {
        prompt_id: "run-034".into(),
        submitted_by: "ai".into(),
        title: Some("改了标题".into()),
        status: "success".into(),
        submitted_at: "2026-09-26T01:00:00Z".into(),
        finished_at: Some("2026-09-26T01:00:05Z".into()),
        elapsed_ms: 5000,
        media_ids: vec![7, 8],
        captured_prompt_id: Some(333),
        error: None,
        message: Some("完成，产物已入库".into()),
    }
    .to_json();
    updated["promptId"] = json!("run-034");
    submissions.record(updated);
    assert_eq!(submissions.list(MAX_TRACKED).len(), before, "就地更新不许多一行");
    let found = submissions.find("run-034").unwrap();
    assert_eq!(found["status"], json!("success"));
    assert_eq!(found["mediaIds"], json!([7, 8]));
    assert_eq!(found["capturedPromptId"], json!(333));
    assert!(submissions.find("不存在").is_none());
}

#[test]
fn 提交记录的形状与_kotlin_的可空约定一致() {
    let queued = Submission {
        prompt_id: "abc".into(),
        submitted_by: "ai".into(),
        title: None,
        status: "queued".into(),
        submitted_at: "2026-09-26T00:00:00Z".into(),
        finished_at: None,
        elapsed_ms: 0,
        media_ids: Vec::new(),
        captured_prompt_id: None,
        error: None,
        message: None,
    }
    .to_json();
    let map = queued.as_object().unwrap();
    // 有默认值的键一律出现（Kotlin `encodeDefaults = true`）
    for key in ["promptId", "submittedBy", "status", "submittedAt", "elapsedMs", "mediaIds"] {
        assert!(map.contains_key(key), "{key} 必须出现：{queued}");
    }
    // 可空键为 null 时整键省略（`explicitNulls = false`）
    for key in ["title", "finishedAt", "capturedPromptId", "error", "message"] {
        assert!(!map.contains_key(key), "{key} 不该出现：{queued}");
    }
}
