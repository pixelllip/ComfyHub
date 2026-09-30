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
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "{\"hello\":\"世界\"}");
    assert_eq!(out.payload["bytes"], std::fs::metadata(&written).unwrap().len() as i64);
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
    assert!(!root.join("storage").join("x.txt").exists(), "越界写被拒了却落了盘");

    // ③ 禁写段：即使用户把白名单放宽到项目根也不行
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
    assert!(forbidden.message.contains("运行期数据 / 依赖目录"), "{}", forbidden.message);

    // ④ append 模式：第一次覆盖、第二次追加
    let first = call_tool(&state, "write_file", serde_json::json!({ "path": "comfyui/a.txt", "content": "甲" }))
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
    assert_eq!(std::fs::read_to_string(root.join("comfyui").join("a.txt")).unwrap(), "甲乙");

    // ⑤ 超过单次上限：**告诉模型怎么分段写**，不是干巴巴一句失败
    let huge = "x".repeat(comfyhub_server_rs::tools::policy::DEFAULT_MAX_WRITE_BYTES as usize + 1);
    let too_large = call_tool(&state, "write_file", serde_json::json!({ "path": "comfyui/big.txt", "content": huge }))
        .await
        .unwrap_err();
    assert_eq!(too_large.code, "TOO_LARGE");
    assert!(too_large.message.contains("mode=\"append\""), "{}", too_large.message);

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
    let first = call_tool(&state, "read_file", serde_json::json!({ "path": "comfyui/flow.json", "maxBytes": 10 }))
        .await
        .unwrap();
    assert!(!first.content.contains('\u{FFFD}'), "页尾出现乱码：{}", first.content);
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
            cursor = chunk.content[pos + 7..].trim_end_matches('）').parse().unwrap();
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
    std::fs::write(root.join("comfyui").join("models").join("b.safetensors"), b"xx").unwrap();

    // 不带 path：默认就是写白名单的第一个（出厂 = comfyui）
    let out = call_tool(&state, "list_dir", serde_json::json!({})).await.unwrap();
    let expected_dir = root.join("comfyui").display().to_string();
    assert!(out.content.starts_with(&expected_dir), "{}", out.content);
    assert!(out.content.contains("file a.json 2B"), "{}", out.content);
    assert!(out.content.contains("dir  models"), "{}", out.content);
    // depth=1 不递归：子目录里的文件不出现
    assert!(!out.content.contains("b.safetensors"), "{}", out.content);
    // depth=2 才列到
    let deep = call_tool(&state, "list_dir", serde_json::json!({ "depth": 2 })).await.unwrap();
    assert!(deep.content.contains("b.safetensors"), "{}", deep.content);

    // 超过 200 条：明说截断（静默截断会让模型以为目录里就这么多东西）
    for i in 0..205 {
        std::fs::write(root.join("comfyui").join(format!("many-{i:03}.txt")), b"x").unwrap();
    }
    let many = call_tool(&state, "list_dir", serde_json::json!({})).await.unwrap();
    assert!(many.content.contains("超过 200 条已截断"), "{}", many.content);

    // 不是目录 → NOT_A_DIRECTORY（码与 Kotlin 一致）
    let not_dir = call_tool(&state, "list_dir", serde_json::json!({ "path": "comfyui/a.json" }))
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
    let out = call_tool(&state, "search_prompts", serde_json::json!({ "query": "cyberpunk" }))
        .await
        .unwrap();
    assert!(out.content.contains("提示词库：命中 1 条"), "{}", out.content);
    assert!(out.content.contains("── [10] 赛博朋克少女"), "{}", out.content);
    assert!(out.content.contains("1girl, cyberpunk"), "要带上正向正文，模型才照抄得了：{}", out.content);
    assert!(out.content.contains("lowres"), "{}", out.content);
    assert!(out.content.contains("tag: 赛博朋克"), "{}", out.content);
    assert!(out.content.contains("anima@0.8"), "{}", out.content);
    assert!(out.content.contains("cfg=3.5"), "{}", out.content);
    assert!(out.content.contains("产物 1 张"), "{}", out.content);
    assert_eq!(out.payload["count"], 1);
    assert_eq!(out.payload["items"][0]["id"], 10);
    assert_eq!(out.payload["items"][0]["mediaCount"], 1);
    assert_eq!(out.payload["items"][0]["tags"], serde_json::json!(["赛博朋克"]));
    assert_eq!(out.payload["items"][0]["positivePrompt"], "1girl, cyberpunk");

    // 按 id 取（与 query 二选一）：拿到的是**同一条**
    let by_id = call_tool(&state, "search_prompts", serde_json::json!({ "id": 11 })).await.unwrap();
    assert!(by_id.content.contains("（id=11）"), "{}", by_id.content);
    assert!(by_id.content.contains("国风山水"), "{}", by_id.content);
    assert_eq!(by_id.payload["items"][0]["positivePrompt"], "chinese ink painting");

    // includePrompt=false：正文与结构化负载里都**不许**再出现提示词正文
    let lean = call_tool(
        &state,
        "search_prompts",
        serde_json::json!({ "id": 11, "includePrompt": "false" }),
    )
    .await
    .unwrap();
    assert!(!lean.content.contains("chinese ink painting"), "{}", lean.content);
    assert!(
        !lean.payload["items"][0].as_object().unwrap().contains_key("positivePrompt"),
        "只想看列表时不该把正文也塞回来：{}",
        lean.payload
    );

    std::fs::remove_dir_all(state.project_root()).ok();
}

#[tokio::test]
async fn search_prompts_搜不到时要说下一步而不是编() {
    let state = state();

    let missing = call_tool(&state, "search_prompts", serde_json::json!({ "query": "不存在的关键字" }))
        .await
        .unwrap_err();
    assert_eq!(missing.code, "NOT_FOUND");
    assert!(missing.message.contains("不存在的关键字"), "{}", missing.message);
    assert!(missing.message.contains("不要"), "要明确禁止库里没有就自己编 tag：{}", missing.message);
    assert!(missing.message.contains("comfy_list_workflows"), "要给另一条出路：{}", missing.message);

    // 按 id 取不到 → 同一个码（`NOT_FOUND`），而不是 500
    let bad_id = call_tool(&state, "search_prompts", serde_json::json!({ "id": 999999 }))
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
        // 配方三件（P4 第 67 步）：`BOTH`/`DESKTOP` 都与 Kotlin 一致 —— 它们都是**桌面专属**，
        // 所以进桌面清单、**不进** run-kit 的手机那份（下面那段断言）
        "comfy_learn_workflow",
        "comfy_list_recipes",
        "comfy_bind_recipe",
    ] {
        assert!(names.contains(&expected), "{expected} 没下发给桌面：{names:?}");
    }
    // 手机那四个也还在
    for expected in ["ask_user_question", "list_skills", "load_skill", "remember"] {
        assert!(names.contains(&expected), "{expected} 丢了：{names:?}");
    }
    // 配方三件（P4 第 67 步）已经实现 → 它们**必须**出现在下发清单里
    // （第 65/66 步那三条 `absent` 断言到这里就该反过来）
    for present in ["comfy_learn_workflow", "comfy_list_recipes", "comfy_bind_recipe"] {
        assert!(names.contains(&present), "{present} 已经搬过来了，不许漏：{names:?}");
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
        assert!(!kit_tools.contains(&absent), "桌面工具漏进了手机 run-kit：{kit_tools:?}");
    }

    // deny 的工具连描述都不下发（与 Kotlin `kitParts` 同一条规则）
    let mut config: comfyhub_server_rs::tools::policy::ToolPolicyConfig = serde_json::from_str("{}").unwrap();
    config.overrides.insert("write_file".into(), "deny".into());
    let denied = comfyhub_server_rs::tools::policy::ToolPolicy::new(state.project_root(), config);
    let denied_names: Vec<String> =
        comfyhub_server_rs::tools::tools::specs(&denied).into_iter().map(|s| s.name).collect();
    assert!(!denied_names.contains(&"write_file".to_string()), "{denied_names:?}");
    assert!(denied_names.contains(&"read_file".to_string()));

    std::fs::remove_dir_all(state.project_root()).ok();
}
