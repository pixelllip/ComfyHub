//! 工具层：桌面文件五件、工具清单与作用域、资料库检索（search_prompts）。
//!
//! 夹具在 [`super::common`]；`use super::common::*;` 之后照旧直接写用例。

use super::common::*;

#[tokio::test]
async fn desktop_file_tools_stay_inside_the_whitelist() {
    let state = state();
    let root = state.project_root();

    // ① 白名单内：真的写下去，结构化负载给 path/bytes
    let out = call_tool(
        &state,
        "write_file",
        serde_json::json!({ "path": "comfyui/sub/工作流.json", "content": "{\"hello\":\"世界\"}" }),
    )
    .await
    .unwrap();
    let written = root.join("comfyui").join("sub").join("工作流.json");
    assert!(written.is_file(), "文件没落盘：{}", written.display());
    assert_eq!(
        std::fs::read_to_string(&written).unwrap(),
        "{\"hello\":\"世界\"}"
    );
    assert_eq!(
        out.payload["bytes"],
        std::fs::metadata(&written).unwrap().len() as i64
    );
    assert_eq!(out.payload["path"], written.display().to_string());

    // ② 越界写：`storage` 是读白名单，不是写白名单
    let denied = call_tool(
        &state,
        "write_file",
        serde_json::json!({ "path": "storage/x.txt", "content": "nope" }),
    )
    .await
    .unwrap_err();
    assert_eq!(denied.code, "PATH_DENIED");
    assert!(
        !root.join("storage").join("x.txt").exists(),
        "越界写被拒了却落了盘"
    );

    // ③ 禁写段：即使白名单放宽到项目根也不允许
    let mut config: comfyhub_server_rs::tools::policy::ToolPolicyConfig =
        serde_json::from_str(r#"{"writeRoots":["."]}"#).unwrap();
    config.write_roots = vec![".".to_string()];
    let wide = comfyhub_server_rs::tools::policy::ToolPolicy::new(root.clone(), config);
    let mut session = comfyhub_server_rs::tools::tools::ToolSession::default();
    let forbidden = comfyhub_server_rs::tools::tools::execute(
        &state,
        &wide,
        "run-x",
        "conv-x",
        "write_file",
        &serde_json::json!({ "path": ".git/hooks/x", "content": "boom" }),
        &mut session,
    )
    .await
    .unwrap_err();
    assert_eq!(forbidden.code, "PATH_DENIED");
    assert!(
        forbidden.message.contains("运行期数据 / 依赖目录"),
        "{}",
        forbidden.message
    );

    // ④ append 模式：第一次覆盖、第二次追加
    let first = call_tool(
        &state,
        "write_file",
        serde_json::json!({ "path": "comfyui/a.txt", "content": "甲" }),
    )
    .await
    .unwrap();
    assert!(first.content.contains("覆盖"), "{}", first.content);
    let second = call_tool(
        &state,
        "write_file",
        serde_json::json!({ "path": "comfyui/a.txt", "content": "乙", "mode": "append" }),
    )
    .await
    .unwrap();
    assert!(second.content.contains("追加"), "{}", second.content);
    assert_eq!(
        std::fs::read_to_string(root.join("comfyui").join("a.txt")).unwrap(),
        "甲乙"
    );

    // ⑤ 超过单次上限：**告诉模型怎么分段写**，不是干巴巴一句失败
    let huge = "x".repeat(comfyhub_server_rs::tools::policy::DEFAULT_MAX_WRITE_BYTES as usize + 1);
    let too_large = call_tool(
        &state,
        "write_file",
        serde_json::json!({ "path": "comfyui/big.txt", "content": huge }),
    )
    .await
    .unwrap_err();
    assert_eq!(too_large.code, "TOO_LARGE");
    assert!(
        too_large.message.contains("mode=\"append\""),
        "{}",
        too_large.message
    );

    std::fs::remove_dir_all(&root).ok();
}

/// 按字节分段读中文：**切点必须落在字符边界上**，而且续读要能拼回原文。
///
/// 这是"模型照着改一份中文工作流"的命门：页尾留一个 U+FFFD，它就会把乱码写回文件。
#[tokio::test]
async fn read_file_splits_chinese_on_character_boundaries() {
    let state = state();
    let root = state.project_root();
    let original = "工作流节点一：加载模型；节点二：正向提示词；节点三：采样器。".repeat(6);
    std::fs::create_dir_all(root.join("comfyui")).unwrap();
    let file = root.join("comfyui").join("flow.json");
    std::fs::write(&file, original.as_bytes()).unwrap();

    // 用小窗口（10 字节）逼出"落在汉字中间"的切点
    let first = call_tool(
        &state,
        "read_file",
        serde_json::json!({ "path": "comfyui/flow.json", "maxBytes": 10 }),
    )
    .await
    .unwrap();
    assert!(
        !first.content.contains('\u{FFFD}'),
        "页尾出现乱码：{}",
        first.content
    );
    let restart = first
        .content
        .rsplit("offset=")
        .next()
        .unwrap()
        .trim_end_matches('）')
        .parse::<usize>()
        .expect("续读提示里必须给出 offset=N");
    assert!(restart > 0 && restart < original.len());

    // 从 offset 接着读，一直读到尾：拼起来必须与原文**逐字相等**
    let mut assembled = first.content.lines().nth(1).unwrap().to_string();
    let mut cursor = restart;
    loop {
        let chunk = call_tool(
            &state,
            "read_file",
            serde_json::json!({ "path": "comfyui/flow.json", "maxBytes": 10, "offset": cursor }),
        )
        .await
        .unwrap();
        let body = chunk.content.lines().nth(1).unwrap().to_string();
        assembled.push_str(&body);
        if let Some(pos) = chunk.content.rfind("offset=") {
            cursor = chunk.content[pos + 7..]
                .trim_end_matches('）')
                .parse()
                .unwrap();
        } else {
            break;
        }
    }
    assert_eq!(assembled, original, "分段读拼回来与原文不一致");
    // 读文件不给结构化负载（Kotlin 那边 meta 也是 null）—— 别自己发明键
    assert!(first.payload.is_null(), "{:?}", first.payload);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn list_dir_defaults_to_the_write_root_and_reports_truncation() {
    let state = state();
    let root = state.project_root();
    std::fs::create_dir_all(root.join("comfyui").join("models")).unwrap();
    std::fs::write(root.join("comfyui").join("a.json"), b"{}").unwrap();
    std::fs::write(
        root.join("comfyui").join("models").join("b.safetensors"),
        b"xx",
    )
    .unwrap();

    // 不带 path：默认就是写白名单的第一个（出厂 = comfyui）
    let out = call_tool(&state, "list_dir", serde_json::json!({}))
        .await
        .unwrap();
    let expected_dir = root.join("comfyui").display().to_string();
    assert!(out.content.starts_with(&expected_dir), "{}", out.content);
    assert!(out.content.contains("file a.json 2B"), "{}", out.content);
    assert!(out.content.contains("dir  models"), "{}", out.content);
    // depth=1 不递归：子目录里的文件不出现
    assert!(!out.content.contains("b.safetensors"), "{}", out.content);
    // depth=2 才列到
    let deep = call_tool(&state, "list_dir", serde_json::json!({ "depth": 2 }))
        .await
        .unwrap();
    assert!(deep.content.contains("b.safetensors"), "{}", deep.content);

    // 超过 200 条：明说截断（静默截断会让模型以为目录里就这么多东西）
    for i in 0..205 {
        std::fs::write(root.join("comfyui").join(format!("many-{i:03}.txt")), b"x").unwrap();
    }
    let many = call_tool(&state, "list_dir", serde_json::json!({}))
        .await
        .unwrap();
    assert!(
        many.content.contains("超过 200 条已截断"),
        "{}",
        many.content
    );

    // 不是目录 → NOT_A_DIRECTORY（码与 Kotlin 一致）
    let not_dir = call_tool(
        &state,
        "list_dir",
        serde_json::json!({ "path": "comfyui/a.json" }),
    )
    .await
    .unwrap_err();
    assert_eq!(not_dir.code, "NOT_A_DIRECTORY");

    std::fs::remove_dir_all(&root).ok();
}

// ===========================================================================
//  资料库：`search_prompts`（O7 —— "写角色之前先查我自己的库"）
//
//  判据不是"能搜到"，而是**搜得到界面搜得到的那一份**：工具与 `/api/prompts`
//  走同一个实现（`routes::query_prompts`），且返回里要带够"照抄"需要的东西
//  （正负提示词 / tag / LoRA / 参数 / 产物数）。少了任何一样，模型就只能凭印象编。
// ===========================================================================

#[tokio::test]
async fn search_prompts_搜得到库里的那一条并带够照抄需要的东西() {
    let state = state();

    // 关键字命中（与 `/api/prompts?q=` 同一条路：标题 / 正负提示词都能命中）
    let out = call_tool(
        &state,
        "search_prompts",
        serde_json::json!({ "query": "cyberpunk" }),
    )
    .await
    .unwrap();
    assert!(
        out.content.contains("提示词库：命中 1 条"),
        "{}",
        out.content
    );
    assert!(
        out.content.contains("── [10] 赛博朋克少女"),
        "{}",
        out.content
    );
    assert!(
        out.content.contains("1girl, cyberpunk"),
        "要带上正向正文，模型才照抄得了：{}",
        out.content
    );
    assert!(out.content.contains("lowres"), "{}", out.content);
    assert!(out.content.contains("tag: 赛博朋克"), "{}", out.content);
    assert!(out.content.contains("anima@0.8"), "{}", out.content);
    assert!(out.content.contains("cfg=3.5"), "{}", out.content);
    assert!(out.content.contains("产物 1 张"), "{}", out.content);
    assert_eq!(out.payload["count"], 1);
    assert_eq!(out.payload["items"][0]["id"], 10);
    assert_eq!(out.payload["items"][0]["mediaCount"], 1);
    assert_eq!(
        out.payload["items"][0]["tags"],
        serde_json::json!(["赛博朋克"])
    );
    assert_eq!(
        out.payload["items"][0]["positivePrompt"],
        "1girl, cyberpunk"
    );

    // 按 id 取（与 query 二选一）：拿到的是**同一条**
    let by_id = call_tool(&state, "search_prompts", serde_json::json!({ "id": 11 }))
        .await
        .unwrap();
    assert!(by_id.content.contains("（id=11）"), "{}", by_id.content);
    assert!(by_id.content.contains("国风山水"), "{}", by_id.content);
    assert_eq!(
        by_id.payload["items"][0]["positivePrompt"],
        "chinese ink painting"
    );

    // includePrompt=false：正文与结构化负载里都**不许**再出现提示词正文
    let lean = call_tool(
        &state,
        "search_prompts",
        serde_json::json!({ "id": 11, "includePrompt": "false" }),
    )
    .await
    .unwrap();
    assert!(
        !lean.content.contains("chinese ink painting"),
        "{}",
        lean.content
    );
    assert!(
        !lean.payload["items"][0]
            .as_object()
            .unwrap()
            .contains_key("positivePrompt"),
        "只想看列表时不该把正文也塞回来：{}",
        lean.payload
    );

    std::fs::remove_dir_all(state.project_root()).ok();
}

#[tokio::test]
async fn search_prompts_搜不到时要说下一步而不是编() {
    let state = state();

    let missing = call_tool(
        &state,
        "search_prompts",
        serde_json::json!({ "query": "不存在的关键字" }),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.code, "NOT_FOUND");
    assert!(
        missing.message.contains("不存在的关键字"),
        "{}",
        missing.message
    );
    assert!(
        missing.message.contains("不要"),
        "要明确禁止库里没有就自己编 tag：{}",
        missing.message
    );
    assert!(
        missing.message.contains("comfy_list_workflows"),
        "要给另一条出路：{}",
        missing.message
    );

    // 按 id 取不到 → 同一个码（`NOT_FOUND`），而不是 500
    let bad_id = call_tool(
        &state,
        "search_prompts",
        serde_json::json!({ "id": 999999 }),
    )
    .await
    .unwrap_err();
    assert_eq!(bad_id.code, "NOT_FOUND");
    assert!(bad_id.message.contains("999999"), "{}", bad_id.message);

    std::fs::remove_dir_all(state.project_root()).ok();
}

/// **R11 / 总纲 §3.5**：桌面专属工具进**桌面**的清单，但**不许**进 run-kit 的 `tools[]`。
#[tokio::test]
async fn desktop_tools_are_declared_for_desktop_but_never_sent_to_the_phone() {
    let state = state();
    let policy = comfyhub_server_rs::tools::policy::ToolPolicy::load(&state).unwrap();

    let specs = comfyhub_server_rs::tools::tools::specs(&policy);
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    for expected in [
        "list_dir",
        "read_file",
        "write_file",
        "register_skill",
        "delete_skill",
        "comfy_get_run",
        "comfy_get_status",
        "comfy_list_workflows",
        "comfy_load_workflow",
        "comfy_sync_history",
        "comfy_use_media",
        "comfy_use_attachment",
        "comfy_make_mask",
        // 看画廊产物（2026-10-29）：只读、不吃 ComfyUI 查询额度
        "comfy_view_outputs",
        "search_prompts",
        "danbooru_lookup",
        // 体检两件（P4 第 62 步）
        "comfy_check_inputs",
        "comfy_fit_workflow",
        // 审核 + 摘拼两件（P4 第 63 步）
        "comfy_audit_workflow",
        "comfy_prune_workflow",
        // 起 / 停实例（2026-09-28）：桌面专属 —— 手机不直接起桌面的实例
        "comfy_start",
        "comfy_stop",
        // 联网三件 + 定向查询（P4 第 64 步）
        "web_search",
        "web_fetch",
        "civitai_search",
        "find_model",
        // 2D 布局模板（2026-11-03）：只出构图草稿，桌面专属（手机没有画布）
        "layout_propose",
        // 配方三件（P4 第 67 步）：`BOTH`/`DESKTOP` 都与 Kotlin 一致 —— 它们都是**桌面专属**，
        // 所以进桌面清单、**不进** run-kit 的手机那份（下面那段断言）
        "comfy_learn_workflow",
        "comfy_list_recipes",
        "comfy_bind_recipe",
    ] {
        assert!(
            names.contains(&expected),
            "{expected} 没下发给桌面：{names:?}"
        );
    }
    // 手机那四个也还在
    for expected in ["ask_user_question", "list_skills", "load_skill", "remember"] {
        assert!(names.contains(&expected), "{expected} 丢了：{names:?}");
    }
    // 配方三件（P4 第 67 步）已经实现 → 它们**必须**出现在下发清单里
    // （第 65/66 步那三条 `absent` 断言到这里就该反过来）
    for present in [
        "comfy_learn_workflow",
        "comfy_list_recipes",
        "comfy_bind_recipe",
    ] {
        assert!(
            names.contains(&present),
            "{present} 已经搬过来了，不许漏：{names:?}"
        );
    }
    // 系统提示里那段"可调用的工具"必须与下发的清单**同一份**
    let catalog = comfyhub_server_rs::tools::tools::catalog(&policy);
    assert_eq!(
        catalog.iter().map(|t| t.name).collect::<Vec<_>>(),
        names,
        "提示词与请求里的工具清单漂移了"
    );

    // run-kit（手机）仍然只有手机可执行的那些（7 个 BOTH + 手机专属的 comfy_queue_submit）
    let (status, kit) = get_json(router_for(state.clone()), "/api/ai/run-kit").await;
    assert_eq!(status, StatusCode::OK, "body={kit}");
    let kit_tools: Vec<&str> = kit["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(kit_tools.len(), 8, "{kit_tools:?}");
    assert!(
        kit_tools.contains(&"comfy_queue_submit"),
        "排队提交必须在手机清单里：不在的话 declaredTo = kit ∩ 本地实现 会把它滤掉，模型看不到它：{kit_tools:?}"
    );
    for absent in [
        "list_dir",
        "read_file",
        "write_file",
        "register_skill",
        "delete_skill",
        "comfy_get_run",
        "comfy_list_workflows",
    ] {
        assert!(
            !kit_tools.contains(&absent),
            "桌面工具漏进了手机 run-kit：{kit_tools:?}"
        );
    }

    // deny 的工具连描述都不下发（与 Kotlin `kitParts` 同一条规则）
    let mut config: comfyhub_server_rs::tools::policy::ToolPolicyConfig =
        serde_json::from_str("{}").unwrap();
    config.overrides.insert("write_file".into(), "deny".into());
    let denied = comfyhub_server_rs::tools::policy::ToolPolicy::new(state.project_root(), config);
    let denied_names: Vec<String> = comfyhub_server_rs::tools::tools::specs(&denied)
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(
        !denied_names.contains(&"write_file".to_string()),
        "{denied_names:?}"
    );
    assert!(denied_names.contains(&"read_file".to_string()));

    std::fs::remove_dir_all(state.project_root()).ok();
}

/// `comfy_diagnose_workflow`（错误接线需在界面指出）：
/// **接线体检**要真的从工具层走得通，并且把"哪根线坏了"说到点上。
///
/// 这条用例**故意不依赖 ComfyUI**（夹具库里没有 `comfyUrl`）：拿不到 `/object_info` 时
/// 它只做结构检查 —— 那正是"不许凭猜下结论"的那一档，而"指向不存在的节点"这条
/// 不需要节点定义就判得出来。
#[tokio::test]
async fn diagnose_workflow_points_at_the_dangling_wire_without_comfyui() {
    let state = state();
    // `2.vae` 指向节点 9 —— 图里没有 9，这是**不需要 /object_info 也判得出来**的接线错误
    let out = call_tool(
        &state,
        "comfy_diagnose_workflow",
        serde_json::json!({
            "graph": {
                "1": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": "a.safetensors" } },
                "2": { "class_type": "VAEDecode", "inputs": { "samples": ["1", 0], "vae": ["9", 0] } }
            }
        }),
    )
    .await
    .unwrap();

    assert_eq!(out.payload["format"], "api", "{}", out.content);
    assert!(
        out.payload["counts"]["errors"].as_i64().unwrap() >= 1,
        "{}",
        out.content
    );
    // 报告要点到**哪个节点、哪个输入**（只报"有问题"等于没说）
    assert!(
        out.content.contains("2.vae"),
        "报告没点到出问题的插口：{}",
        out.content
    );
    assert!(
        out.content.contains('9'),
        "报告没点到那个不存在的节点：{}",
        out.content
    );
    // ⚠ 不断言"没连上 ComfyUI"那句话：**本机真的跑着 ComfyUI** 时它会连上、覆盖面就变大了
    //   （那是环境差异，不是行为差异）。"拿不到 /object_info 就只做结构检查"由
    //   `workflow_blueprint::tests` 那两条钉着（一条离线、一条在线）。
    assert!(out.payload["objectInfo"].is_boolean(), "{}", out.content);
}

/// 体检走的是**同一份路径准入**（读白名单之外的文件一律拒），
/// 而且拒绝要说人话（不是 500 / 不是 TOOL_NOT_FOUND）。
#[tokio::test]
async fn diagnose_workflow_respects_the_read_whitelist() {
    let state = state();
    let denied = call_tool(
        &state,
        "comfy_diagnose_workflow",
        serde_json::json!({ "path": "C:\\Windows\\win.ini" }),
    )
    .await
    .unwrap_err();
    assert_eq!(denied.code, "PATH_DENIED", "{}", denied.message);

    // 三个来源一个都不给：如实说"要给我一份工作流"，别猜
    let empty = call_tool(&state, "comfy_diagnose_workflow", serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(empty.code, "INVALID_ARGUMENT", "{}", empty.message);
}

/// `comfy_view_outputs`（"能否让 AI 查看直接在 ComfyUI 中运行的产物"）：
/// 读画廊最近的图片 → 返回 `referenceImages`（执行器据此自动附图），并且在正文里给出
/// id / 尺寸 / 关联提示词 —— **无需再次发送图片**。
///
/// 这里只验工具层；"图真的挂进下一轮"由执行器的 `reference_attach` 那一份负责
/// （能力判据 / 预算 / 去重都在它那里，工具不重复实现）。
#[tokio::test]
async fn view_outputs_returns_recent_media_as_reference_images() {
    let state = state();
    write_test_png(&state.media_dir().join("shot.png"), 320, 200);
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO media_assets
                   (id, prompt_id, kind, original_name, stored_name, width, height, created_at, source)
                 VALUES (101, 10, 'IMAGE', 'shot.png', 'shot.png', 320, 200,
                         '2026-10-29T10:00:00.000Z', 'capture')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let out = call_tool(
        &state,
        "comfy_view_outputs",
        serde_json::json!({ "limit": 5 }),
    )
    .await
    .unwrap();
    // 夹具里还有一条 a.png（id=100）——两条都要列出来
    assert_eq!(out.payload["count"], 2, "{}", out.content);
    let refs = out.payload["referenceImages"].as_array().unwrap();
    let shot = refs
        .iter()
        .find(|r| r["file"] == "shot.png")
        .unwrap_or_else(|| panic!("新产物没进 referenceImages：{}", out.content));
    assert_eq!(shot["label"], "产物 #101");
    assert_eq!(shot["exists"], true);
    assert!(shot["path"].as_str().unwrap().ends_with("shot.png"));
    // 正文要带上关联提示词：模型看到图之后还得知道"这张是按什么提示词跑的"
    assert!(out.content.contains("1girl, cyberpunk"), "{}", out.content);

    // 指定 mediaId：只看这一条
    let one = call_tool(
        &state,
        "comfy_view_outputs",
        serde_json::json!({ "mediaId": 101 }),
    )
    .await
    .unwrap();
    assert_eq!(one.payload["count"], 1);
    assert_eq!(one.payload["items"][0]["id"], 101);

    // 不存在：**空结果 + 一句下一步**，不要报成工具错误让模型反复重试
    let none = call_tool(
        &state,
        "comfy_view_outputs",
        serde_json::json!({ "mediaId": 999 }),
    )
    .await
    .unwrap();
    assert_eq!(none.payload["count"], 0);
    assert!(none.content.contains("没有"), "{}", none.content);

    // 翻页（"不能只看前 x 张"）：列表必须能沿 `offset` 查看更早的，
    // 并且结果里带 `total` / `hasMore` —— 否则模型只能"看完第一页就说就这些"。
    state
        .with(|conn| {
            // 夹具里 100 那条是默认时间（= 现在），压到很早，让顺序可断言
            conn.execute(
                "UPDATE media_assets SET created_at = '2026-09-01T00:00:00.000Z' WHERE id = 100",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    for (id, name, at) in [
        (201, "p1.png", "2026-11-09T10:00:01.000Z"),
        (202, "p2.png", "2026-11-09T10:00:02.000Z"),
        (203, "p3.png", "2026-11-09T10:00:03.000Z"),
    ] {
        write_test_png(&state.media_dir().join(name), 4, 4);
        state
            .with(|conn| {
                conn.execute(
                    "INSERT INTO media_assets
                       (id, prompt_id, kind, original_name, stored_name, width, height, created_at)
                     VALUES (?1, 11, 'IMAGE', ?2, ?2, 4, 4, ?3)",
                    rusqlite::params![id, name, at],
                )?;
                Ok(())
            })
            .unwrap();
    }

    let page1 = call_tool(
        &state,
        "comfy_view_outputs",
        serde_json::json!({ "limit": 2 }),
    )
    .await
    .unwrap();
    assert_eq!(page1.payload["count"], 2, "{}", page1.content);
    assert_eq!(page1.payload["total"], 5, "{}", page1.content);
    assert_eq!(page1.payload["hasMore"], true, "{}", page1.content);
    assert_eq!(page1.payload["items"][0]["id"], 203);
    assert_eq!(page1.payload["items"][1]["id"], 202);
    assert!(
        page1.content.contains("第 1-2 张") && page1.content.contains("共 5 张"),
        "正文要说清这是第几页、总共有几张：{}",
        page1.content
    );

    let page2 = call_tool(
        &state,
        "comfy_view_outputs",
        serde_json::json!({ "limit": 2, "offset": 2 }),
    )
    .await
    .unwrap();
    assert_eq!(page2.payload["offset"], 2);
    assert_eq!(page2.payload["items"][0]["id"], 201);
    assert_eq!(page2.payload["items"][1]["id"], 101);
    assert_eq!(page2.payload["hasMore"], true, "{}", page2.content);
    assert!(
        page2.content.contains("offset=4"),
        "正文要告诉模型下一页从哪翻：{}",
        page2.content
    );

    let page3 = call_tool(
        &state,
        "comfy_view_outputs",
        serde_json::json!({ "limit": 2, "offset": 4 }),
    )
    .await
    .unwrap();
    assert_eq!(page3.payload["count"], 1);
    assert_eq!(page3.payload["items"][0]["id"], 100);
    assert_eq!(page3.payload["hasMore"], false, "{}", page3.content);

    // 按一次运行的 promptId 过滤：只回这次生成的 3 张（夹具里 100 属于 prompt 10）
    let by_prompt = call_tool(
        &state,
        "comfy_view_outputs",
        serde_json::json!({ "promptId": 11, "limit": 50 }),
    )
    .await
    .unwrap();
    assert_eq!(by_prompt.payload["total"], 3, "{}", by_prompt.content);
    assert!(
        by_prompt.payload["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["id"] != 100),
        "{}",
        by_prompt.content
    );

    // 翻过头：空结果 + 如实说总数，不报工具错误
    let too_far = call_tool(
        &state,
        "comfy_view_outputs",
        serde_json::json!({ "offset": 99 }),
    )
    .await
    .unwrap();
    assert_eq!(too_far.payload["count"], 0);
    assert_eq!(too_far.payload["total"], 5);
    assert!(too_far.content.contains("超出范围"), "{}", too_far.content);
}

/// 2026-11-03：`layout_propose` 的**执行接线**（不只是纯函数）。
///
/// 盯的是工具层这一跳：`execute` 真的有这条分支、结果进 `payload.layoutTemplates`、
/// 越界框被夹回并如实写进 `issues`，而且**正文不再复述坐标**。
#[tokio::test]
async fn layout_propose_returns_editable_templates_and_fixes_boxes() {
    let state = state();
    let out = call_tool(
        &state,
        "layout_propose",
        serde_json::json!({
            "brief": "雾夜废墟，一名提灯的旅人正走近教堂",
            "templates": [{
                "title": "三分法：旅人压左",
                "width": 768,
                "height": 1024,
                "camera": "eye",
                "regions": [
                    { "label": "提灯的旅人", "kind": "character",
                      "x": 40, "y": 300, "w": 260, "h": 640, "surface": "floor" },
                    // 900+400 越界：必须被夹回 600
                    { "label": "教堂", "kind": "object",
                      "x": 900, "y": 120, "w": 400, "h": 760 },
                    { "label": "镜中的她", "kind": "character",
                      "x": 700, "y": 200, "w": 120, "h": 200,
                      "relation": "mirror", "relationTo": "r1" }
                ]
            }]
        }),
    )
    .await
    .unwrap();

    let templates = out.payload["layoutTemplates"].as_array().expect("要有模板");
    assert_eq!(templates.len(), 1, "{}", out.content);
    let regions = templates[0]["spec"]["regions"].as_array().unwrap();
    assert_eq!(regions.len(), 3);
    assert_eq!(regions[1]["x"], 600, "越界框必须夹回界内");
    assert_eq!(regions[2]["relation"], "mirror");
    assert_eq!(regions[2]["relationTo"], "r1");
    assert!(
        out.payload["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i.as_str().unwrap_or("").contains("越出画布")),
        "修正要如实记账：{}",
        out.payload["issues"]
    );
    // 正文给模型看的是摘要，不重复贴整份坐标（坐标是它自己写的）
    assert!(out.content.contains("三分法：旅人压左"), "{}", out.content);
    assert!(
        !out.content.contains("\"regions\""),
        "正文不该复述 JSON：{}",
        out.content
    );

    std::fs::remove_dir_all(state.project_root()).ok();
}

/// 2026-11-11：写完提示词后按它生成**一个**构图核对模板；提交同一版提示词才放行。
#[tokio::test]
async fn layout_verify_template_is_required_before_submitting_its_prompt() {
    let state = state();
    let mut session = comfyhub_server_rs::tools::tools::ToolSession::default();
    let prompt = "1girl, standing left of the church, lantern in hand";

    // 没生成核对模板 → 提交在工具层就被拒，不碰 ComfyUI。
    let err = call_tool_with_session(
        &state,
        "comfy_submit",
        serde_json::json!({ "promptId": 1, "positivePrompt": prompt }),
        &mut session,
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "LAYOUT_TEMPLATE_REQUIRED", "{}", err.message);

    // 生成之后，同一版提示词（只差空白）能过闸门；后面的失败原因是"库里没有这份图"。
    let out = call_tool_with_session(
        &state,
        "layout_propose",
        serde_json::json!({
            "purpose": "verify",
            "prompt": prompt,
            "brief": "雾夜废墟",
            "templates": [{
                "title": "三分法：旅人压左",
                "width": 768,
                "height": 1024,
                "regions": [
                    { "label": "提灯的旅人", "kind": "character",
                      "x": 40, "y": 300, "w": 260, "h": 640 }
                ]
            }]
        }),
        &mut session,
    )
    .await
    .unwrap();
    assert_eq!(out.payload["layoutTemplates"][0]["purpose"], "verify");

    // 换了一版提示词 → 模板对不上，必须重新生成。
    let err = call_tool_with_session(
        &state,
        "comfy_submit",
        serde_json::json!({ "promptId": 1, "positivePrompt": "1girl, lantern" }),
        &mut session,
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "LAYOUT_TEMPLATE_MISMATCH", "{}", err.message);

    // 同一版提示词 → 过了核对闸门（后面的报错是"库里没有这份图"，与本功能无关）。
    let err = call_tool_with_session(
        &state,
        "comfy_submit",
        serde_json::json!({ "promptId": 1, "positivePrompt": prompt }),
        &mut session,
    )
    .await
    .unwrap_err();
    assert_ne!(err.code, "LAYOUT_TEMPLATE_REQUIRED", "{}", err.message);
    assert_ne!(err.code, "LAYOUT_TEMPLATE_MISMATCH", "{}", err.message);

    std::fs::remove_dir_all(state.project_root()).ok();
}
