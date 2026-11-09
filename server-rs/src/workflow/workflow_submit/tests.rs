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
    assert!(WorkflowEdit::apply_overrides(&graph(), &bad_path)
        .unwrap_err()
        .contains("节点id.输入名"));

    let mut no_node = Map::new();
    no_node.insert("99.steps".into(), json!(1));
    assert!(WorkflowEdit::apply_overrides(&graph(), &no_node)
        .unwrap_err()
        .contains("没有节点 99"));

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
    with_hole["6"]["inputs"]
        .as_object_mut()
        .unwrap()
        .remove("clip");
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
    assert!(WorkflowEdit::apply_connections(&graph(), &bad_shape)
        .unwrap_err()
        .contains("两个元素"));

    let mut bad_slot = Map::new();
    bad_slot.insert("6.clip".into(), json!(["4", "第一个"]));
    assert!(WorkflowEdit::apply_connections(&graph(), &bad_slot)
        .unwrap_err()
        .contains("槽位要是整数"));

    // 数字 id 与字符串 id 必须都能用（真库里有 `"negative": [76, 0]` 这种）
    // —— 但**写进图里的引用一律是字符串**：ComfyUI 拿它索引 prompt 字典，数字会 KeyError。
    let mut numeric = Map::new();
    numeric.insert("6.clip".into(), json!([4, 1]));
    let (patched, _) = WorkflowEdit::apply_connections(&graph(), &numeric).unwrap();
    assert_eq!(patched["6"]["inputs"]["clip"], json!(["4", 1]));
}

/// 正面提示词直接写进最近的 `CLIPTextEncode.text`，**不碰 negative**，
/// 并把 TextGenerate 从提交图里删掉（不是只断线）。
#[test]
fn positive_prompt_writes_positive_text_and_removes_text_generate() {
    let graph = json!({
        "10": {"class_type": "KSampler", "inputs": {
            "positive": ["6", 0], "negative": ["20", 0], "seed": 1
        }},
        "6": {"class_type": "CLIPTextEncode", "inputs": {
            "text": ["7", 0], "clip": ["4", 0]
        }},
        "7": {"class_type": "TextGenerate", "inputs": {
            "system_prompt": "把想法写成 caption", "prompt": "一只猫"
        }},
        "20": {"class_type": "CLIPTextEncode", "inputs": {"text": "low quality"}},
        "30": {"class_type": "SaveImage", "inputs": {"images": ["10", 0]}}
    });
    let (patched, changed, removed) =
        WorkflowEdit::apply_positive_prompt(&graph, "  A photo of a cat on a windowsill.  ")
            .unwrap();
    assert_eq!(
        patched["6"]["inputs"]["text"],
        "A photo of a cat on a windowsill."
    );
    assert_eq!(
        patched["20"]["inputs"]["text"], "low quality",
        "negative 一个字都不许动"
    );
    assert_eq!(changed, vec!["6.text".to_string()]);
    assert_eq!(
        removed,
        vec!["7".to_string()],
        "要如实回报从提交图里摘掉了谁"
    );
    assert!(
        patched.get("7").is_none(),
        "TextGenerate 必须从提交图里消失（留在图里仍会被整张校验）：{patched}"
    );
    // 原图不能改（纯函数）
    assert_eq!(graph["6"]["inputs"]["text"], json!(["7", 0]));
    assert!(graph.get("7").is_some(), "原图里的节点不许删");
}

/// MOSS t2i 那种 `CLIPTextEncode.text ← PreviewAny ← ComfySwitchNode` 的链：
/// 断线的同时把整支（含只喂它的上游与指向它的显示节点）摘干净，不留悬挂引用。
#[test]
fn positive_prompt_removes_the_whole_switch_and_preview_branch() {
    let graph = json!({
        "36": {"class_type": "KSampler", "inputs": {"positive": ["34", 0], "negative": ["41", 0]}},
        "34": {"class_type": "CLIPTextEncode", "inputs": {"text": ["38", 0], "clip": ["35", 0]}},
        "38": {"class_type": "PreviewAny", "inputs": {"source": ["39", 0]}},
        "39": {"class_type": "ComfySwitchNode", "inputs": {
            "on_false": ["30", 0], "on_true": ["32", 0], "switch": false
        }},
        "30": {"class_type": "PrimitiveStringMultiline", "inputs": {"value": "用户原话"}},
        "32": {"class_type": "TextGenerate", "inputs": {"system_prompt": ["31", 0]}},
        "31": {"class_type": "PrimitiveStringMultiline", "inputs": {"value": "扩写指令"}},
        "35": {"class_type": "SaveImage", "inputs": {"images": ["34", 0]}},
        "41": {"class_type": "CLIPTextEncode", "inputs": {"text": ""}}
    });
    let (patched, changed, removed) =
        WorkflowEdit::apply_positive_prompt(&graph, "最终提示词").unwrap();
    assert_eq!(patched["34"]["inputs"]["text"], "最终提示词");
    assert_eq!(changed, vec!["34.text".to_string()]);
    assert_eq!(
        removed,
        vec![
            "31".to_string(),
            "32".to_string(),
            "38".to_string(),
            "39".to_string()
        ],
        "TextGenerate + 专属上游 + 指向它的显示节点要一起摘"
    );
    for id in ["31", "32", "38", "39"] {
        assert!(patched.get(id).is_none(), "节点 {id} 还在提交图里");
    }
    // 30 只是失去了消费者（原本喂 switch 的用户原话），不在 TG 的祖先链上；
    // 留在图里既不会被执行也不会让校验失败 —— 不为了"干净"去猜它该不该删。
    // 没有悬挂引用：剩下的每个连线都指向还在图里的节点
    for (id, node) in patched.as_object().unwrap() {
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        for (name, value) in inputs {
            if let Some((up, _)) = link_ref(value) {
                assert!(
                    patched.get(&up).is_some(),
                    "{id}.{name} 还指向被摘掉的 {up}"
                );
            }
        }
    }
    assert_eq!(patched["41"]["inputs"]["text"], "", "negative 不动");
}

/// 摘除要保底：**不能把最后一个输出摘掉** —— 那种图退回"只断线"，并如实回报没摘谁。
#[test]
fn positive_prompt_keeps_text_generate_when_removal_would_leave_no_output() {
    let graph = json!({
        "36": {"class_type": "KSampler", "inputs": {"positive": ["34", 0]}},
        "34": {"class_type": "CLIPTextEncode", "inputs": {"text": ["7", 0]}},
        "7": {"class_type": "TextGenerate", "inputs": {"prompt": "一只猫"}}
    });
    let (patched, _, removed) = WorkflowEdit::apply_positive_prompt(&graph, "最终提示词").unwrap();
    assert_eq!(
        patched["34"]["inputs"]["text"], "最终提示词",
        "正文照样要写进去"
    );
    // 这张图本来就没输出节点（没有 Save/Preview）→ 摘除会让图彻底没法提交，于是不摘。
    assert!(removed.is_empty(), "没有输出时不许摘：{removed:?}");
}

/// `BasicGuider` 没有 `positive` 插口，只有 `conditioning` —— Krea 2 的
/// `SamplerCustomAdvanced → BasicGuider → CLIPTextEncode` 必须也能找到。
#[test]
fn positive_prompt_follows_basic_guider_conditioning() {
    let graph = json!({
        "40": {"class_type": "SamplerCustomAdvanced", "inputs": {
            "guider": ["41", 0], "noise": ["42", 0]
        }},
        "41": {"class_type": "BasicGuider", "inputs": {
            "model": ["1", 0], "conditioning": ["34", 0]
        }},
        "34": {"class_type": "CLIPTextEncode", "inputs": {"text": "old", "clip": ["2", 0]}},
        "42": {"class_type": "RandomNoise", "inputs": {"noise_seed": 1}}
    });
    let (patched, changed, _) = WorkflowEdit::apply_positive_prompt(&graph, "krea prompt").unwrap();
    assert_eq!(patched["34"]["inputs"]["text"], "krea prompt");
    assert_eq!(changed, vec!["34.text".to_string()]);
}

/// 找不到正面文本编码器 / 空提示词都要报错，**不许静默什么都不改**。
#[test]
fn positive_prompt_refuses_when_it_cannot_find_a_target() {
    let no_positive = json!({"1": {"class_type": "SaveImage", "inputs": {}}});
    let message = WorkflowEdit::apply_positive_prompt(&no_positive, "x").unwrap_err();
    assert!(message.contains("positive"), "{message}");

    let graph = json!({
        "10": {"class_type": "KSampler", "inputs": {"positive": ["6", 0]}},
        "6": {"class_type": "ConditioningCombine", "inputs": {"conditioning_1": ["7", 0]}},
        "7": {"class_type": "SomeOp", "inputs": {}}
    });
    let message = WorkflowEdit::apply_positive_prompt(&graph, "x").unwrap_err();
    assert!(message.contains("CLIPTextEncode"), "{message}");

    let message = WorkflowEdit::apply_positive_prompt(&graph, "   ").unwrap_err();
    assert!(message.contains("空"), "{message}");
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
    assert_eq!(
        negative_capability(&only_positive, Some(&info_with_negative())),
        Some(Vec::new())
    );
}

#[test]
fn 负面能力_拿不到_object_info_就回_none_不猜() {
    let graph = json!({ "3": { "class_type": "KSampler", "inputs": { "negative": ["7", 0] } } });
    assert_eq!(
        negative_capability(&graph, None),
        None,
        "ComfyUI 没跑（拿不到 /object_info）时不许说'这份图不认负面'"
    );
}

#[test]
fn 预检_节点类拦_模型文件只警告() {
    assert!(
        preflight_refusal(&graph(), &object_info()).is_none(),
        "都对就必须放行"
    );

    // ① 实例不认识的节点类 → **拦**（`/object_info` 对节点类是权威的）
    let mut missing_node = graph();
    missing_node["12"] = json!({ "class_type": "Krea2CustomNode", "inputs": {} });
    let refusal = preflight_refusal(&missing_node, &object_info()).unwrap();
    assert!(refusal.contains("实例里没有这些节点类"), "{refusal}");
    assert!(refusal.contains("12: Krea2CustomNode"), "{refusal}");
    assert!(
        refusal.contains("不要") && refusal.contains("改用户的工作流"),
        "{refusal}"
    );

    // ② 模型文件不在实例的候选里 → **只警告不拦**（见 `preflight_refusal` 的说明：
    //    盘扫还没搬，只按候选判会误拦；e2e-submit 第 4/5 幕就是活证）
    let mut wrong_model = graph();
    wrong_model["4"]["inputs"]["ckpt_name"] = json!("anima-base-v1.0.safetensors");
    assert!(
        preflight_refusal(&wrong_model, &object_info()).is_none(),
        "模型文件那一档现在是提示，不许拦"
    );
    let warnings = preflight_warnings(&wrong_model, &object_info());
    assert_eq!(
        warnings.len(),
        3,
        "首行 + 一条明细 + 一条动作：{warnings:?}"
    );
    let joined = warnings.join("\n");
    // ⚠ 这一档的措辞 2026-09-30 由用户自己精简过（去掉了"没有拦""重启 ComfyUI""别去下载"）。
    //   **"先让用户重启实例 / 修模型根、别去下载"那条纪律现在只由工具描述承担**
    //   （`desktop_tools` 里 `comfy_submit` / `comfy_env_report` 的描述 + 系统提示），
    //   所以这里只钉"这条警告本身必须说清的事"：**看不见几个、是哪一个、去哪儿看全**。
    assert!(
        joined.contains("看不到 1 个模型文件"),
        "要说清数量：{joined}"
    );
    assert!(
        joined.contains("4.ckpt_name 要 anima-base-v1.0.safetensors"),
        "{joined}"
    );
    assert!(
        joined.contains("comfy_env_report"),
        "要给下一步去哪儿看全：{joined}"
    );
    // 都对的时候没有警告
    assert!(preflight_warnings(&graph(), &object_info()).is_empty());

    // ③ 非加载器节点的候选列表**不参与**判定（图片输入那种动态列表会误报）
    let mut with_image = graph();
    with_image["9"] =
        json!({ "class_type": "LoadImage", "inputs": { "image": "not-in-list.png" } });
    let mut info = object_info();
    info["LoadImage"] = json!({ "input": { "required": { "image": [["a.png"], {}] } } });
    assert!(
        preflight_refusal(&with_image, &info).is_none(),
        "LoadImage 不是 Loader，不判它"
    );
    assert!(
        preflight_warnings(&with_image, &info).is_empty(),
        "也不该警告"
    );
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
    let report = workflow_audit::audit(frontend.as_object().unwrap(), info.as_object(), None);
    assert!(
        report.blocking_refusal().is_some(),
        "纯前端节点必须由审核拦下：{}",
        report.render()
    );
    // 判据本身（三张表/模式的合取）钉一下
    for cls in [
        "Label (rgthree)",
        "Anything Everywhere",
        "PreviewImage",
        "Display Any",
    ] {
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
    assert_eq!(
        listed[0]["promptId"],
        json!(format!("run-{:03}", MAX_TRACKED + 4)),
        "新的在前"
    );

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
    assert_eq!(
        submissions.list(MAX_TRACKED).len(),
        before,
        "就地更新不许多一行"
    );
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
    for key in [
        "promptId",
        "submittedBy",
        "status",
        "submittedAt",
        "elapsedMs",
        "mediaIds",
    ] {
        assert!(map.contains_key(key), "{key} 必须出现：{queued}");
    }
    // 可空键为 null 时整键省略（`explicitNulls = false`）
    for key in [
        "title",
        "finishedAt",
        "capturedPromptId",
        "error",
        "message",
    ] {
        assert!(!map.contains_key(key), "{key} 不该出现：{queued}");
    }
}

/// `textGenerateSkill` 只改**这一次提交的图**：写进 `TextGenerate.system_prompt` 指向的字符串节点。
#[test]
fn 技能正文写进_text_generate_的系统提示节点() {
    let graph = json!({
        "43": { "class_type": "TextGenerate", "inputs": { "prompt": ["182", 0], "system_prompt": ["45", 0] } },
        "45": { "class_type": "PrimitiveStringMultiline", "inputs": { "value": "旧系统提示" } },
        "182": { "class_type": "StringConcatenate", "inputs": { "string_a": "a", "string_b": "b" } }
    });
    let (patched, targets) = apply_text_generate_system_prompt(&graph, "新的 skill 正文").unwrap();
    assert_eq!(patched["45"]["inputs"]["value"], json!("新的 skill 正文"));
    assert_eq!(targets, vec!["45.value".to_string()]);
    // 原图不动
    assert_eq!(graph["45"]["inputs"]["value"], json!("旧系统提示"));
}

/// 没有 TextGenerate（或它接的不是字符串节点）要报出来，不许静默什么都不改。
#[test]
fn 技能正文找不到可写节点时报错() {
    let graph = json!({
        "43": { "class_type": "TextGenerate", "inputs": { "prompt": ["182", 0] } }
    });
    let err = apply_text_generate_system_prompt(&graph, "x").unwrap_err();
    assert!(err.contains("TextGenerate.system_prompt"), "{err}");
}

/// `promptSource` 的判据：正面文本是从 `TextGenerate` 那条链上来的才算生成。
#[test]
fn 正面条件是否由_text_generate_生成() {
    let generated = json!({
        "128": { "class_type": "KSampler", "inputs": { "positive": ["108", 0] } },
        "108": { "class_type": "CLIPTextEncode", "inputs": { "text": ["162", 0] } },
        "162": { "class_type": "ComfySwitchNode", "inputs": { "on_true": ["43", 0], "on_false": ["182", 0] } },
        "43": { "class_type": "TextGenerate", "inputs": { "prompt": ["182", 0] } },
        "182": { "class_type": "StringConcatenate", "inputs": { "string_a": "a", "string_b": "b" } }
    });
    assert!(positive_prompt_is_generated(&generated));

    let literal = json!({
        "128": { "class_type": "KSampler", "inputs": { "positive": ["108", 0] } },
        "108": { "class_type": "CLIPTextEncode", "inputs": { "text": "写死的提示词" } }
    });
    assert!(!positive_prompt_is_generated(&literal));
}

/// T2（2026-11-09）：已验证回写**只认槽位键** —— 旋钮（步数 / 提示词）不是"这个模型能用"的证据。
#[test]
fn 已验证回写只认槽位键_不把旋钮当成模型验证() {
    let recipe =
        crate::workflow::workflow_recipe::analyze(graph().as_object().unwrap(), None);
    let mut overrides = Map::new();
    overrides.insert("4.ckpt_name".into(), json!("new.safetensors"));
    overrides.insert("3.steps".into(), json!(30));
    overrides.insert("6.text".into(), json!("新提示词"));
    overrides.insert("4.莫名输入".into(), json!("不是槽位"));
    let entries = verified_entries(&recipe, &overrides, "run-1", Some("2026-11-09T02:00:00Z".into()));
    assert_eq!(entries.len(), 1, "只有槽位键算验证：{entries:?}");
    assert_eq!(entries[0].override_key, "4.ckpt_name");
    assert_eq!(entries[0].model, "new.safetensors");
    assert_eq!(entries[0].role, "checkpoints");
    assert_eq!(entries[0].run_key, "run-1");
    assert_eq!(entries[0].at.as_deref(), Some("2026-11-09T02:00:00Z"));

    // 没改槽位（只改参数）= 没验证任何模型；空白文件名同样不算
    assert!(verified_entries(&recipe, &Map::new(), "run-2", None).is_empty());
    let mut blank = Map::new();
    blank.insert("4.ckpt_name".into(), json!("   "));
    assert!(verified_entries(&recipe, &blank, "run-3", None).is_empty());
}
