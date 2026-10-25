//! run-kit 工件与 Skills / 记忆：下发规则、版本与开关、skill 的读写与自注册。
//!
//! 夹具在 [`super::common`]；`use super::common::*;` 之后照旧直接写用例。

use super::common::*;


/// run-kit 的形状：11 个顶层键、7 个手机工具、模板横幅、两条按轮指令、预算数值。
#[tokio::test]
async fn run_kit_ships_every_rule_the_phone_needs() {
    let state = state();
    seed_run_kit(&state);
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/run-kit").await;
    assert_eq!(status, StatusCode::OK, "body={body}");

    assert_eq!(
        keys(&body),
        vec![
            "continueInstruction", "inlineBudget", "kitVersion", "models", "nsfwEnabled",
            "promptVersion", "schemaVersion", "systemPrompt", "titleInstruction", "tools",
            "untrustedBanner",
        ]
    );
    assert_eq!(body["schemaVersion"], 1, "与 /api/sync/manifest 的 user_version 同一个数");
    // v51：23④ 按识图能力分流 + 表情不许来自角色卡。
    assert_eq!(body["promptVersion"], "v51");
    assert_eq!(body["nsfwEnabled"], false, "夹具里没开 NSFW");
    assert_eq!(body["inlineBudget"]["perImageBytes"], 8388608, "单图 8MB");
    assert_eq!(body["inlineBudget"]["totalBytes"], 20971520, "合计 20MB");
    let kit = body["kitVersion"].as_str().unwrap();
    assert!(kit.contains('.'), "kitVersion 形如 <日期>.<当日序号>：{kit}");

    // tools[]：**只有手机可执行的那 8 个**（7 个 BOTH + 手机专属的 `comfy_queue_submit`），每项恰好四个键
    let tools = body["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "ask_user_question", "civitai_search", "comfy_queue_submit", "list_skills", "load_skill",
            "remember", "web_fetch", "web_search",
        ],
        "按 name 升序（顺序进了 kitVersion 签名）"
    );
    for tool in tools {
        assert_eq!(keys(tool), vec!["description", "inputSchema", "name", "scope"]);
        assert_eq!(tool["inputSchema"]["type"], "object");
        let scope = tool["scope"].as_str().unwrap();
        assert!(scope == "MOBILE" || scope == "BOTH", "错档：{scope}");
    }
    // 桌面专属的一个都不许出现（总纲 §3.5：别让模型看见它调不到的工具）
    for forbidden in [
        "comfy_submit", "comfy_get_status", "read_file", "write_file", "register_skill",
        "delete_skill", "list_dir", "find_model", "comfy_load_workflow",
    ] {
        assert!(!names.contains(&forbidden), "{forbidden} 是桌面专属，不该进手机清单");
    }

    // 横幅是**模板**（R7：两端不许各写一份），两条按轮指令也必须下发（R13）
    let banner = body["untrustedBanner"].as_str().unwrap();
    assert!(banner.contains("{kind}"));
    assert!(banner.contains("\n查询：{query}"));
    assert!(body["titleInstruction"].as_str().unwrap().contains("[标题]"));
    assert!(body["continueInstruction"]
        .as_str()
        .unwrap()
        .starts_with("（上一条回复因为输出长度上限被截断了。）"));

    // systemPrompt：五段来源都要真的进去
    let prompt = body["systemPrompt"].as_str().unwrap();
    assert!(prompt.contains("- Provider: 本机网关（openai-completions）"));
    assert!(prompt.contains("- Model: m-full"), "缺省取第一个启用模型（display_name, model_id 序）");
    assert!(prompt.contains("- 本次请求附带图片附件: 无"), "kit 是取那一刻的工件");
    assert!(prompt.contains("demo-skill"), "已装 Skills 的目录要进去");
    assert!(prompt.contains("用户偏好 4:3 画幅"), "长期记忆要进去");
    assert!(prompt.contains("现有 1/100 条"), "还要报占用（模型据此知道还剩多少额度）");
    assert!(prompt.contains("**只能**写"), "文件白名单要如实写出来");
    assert!(prompt.contains("- 可调用的工具（参数结构见接口定义）：ask_user_question、"));
    assert!(prompt.contains("事实纪律（不要猜本机状态）"), "常驻段必须在");
    assert!(!prompt.contains("[MODE: UNRESTRICTED SANDBOX"), "NSFW 关着就不许注入那段");
    assert!(prompt.contains("本机没有开启 NSFW 模式"), "关掉时要**如实说明**");

    // models[]：只有启用的两个；12 个键永远在，5 个可空键按行给
    let models = body["models"].as_array().unwrap();
    assert_eq!(models.len(), 2, "关掉的模型不许出现");
    assert_eq!(models[0]["modelId"], "m-full");
    assert_eq!(models[0]["providerName"], "本机网关");
    assert_eq!(models[0]["providerApi"], "openai-completions");
    assert_eq!(
        keys(&models[0]),
        vec![
            "attachmentTransports", "capabilitySource", "contextWindow", "displayName",
            "inputModalities", "maxAttachmentBytes", "maxAttachmentCount", "maxOutputTokens",
            "modelId", "parallelTools", "providerApi", "providerId", "providerName", "reasoning",
            "thinkingEfforts", "thinkingFormat", "tools",
        ]
    );
    assert_eq!(
        keys(&models[1]),
        vec![
            "attachmentTransports", "capabilitySource", "displayName", "inputModalities",
            "modelId", "parallelTools", "providerApi", "providerId", "providerName", "reasoning",
            "thinkingEfforts", "tools",
        ],
        "可空列为 NULL 时那 5 个键一个都不许自己冒出来"
    );
    // 桌面的凭据引用（`credentialRef`）连名字都不该下发
    let raw = serde_json::to_string(&body).unwrap();
    assert!(!raw.contains("RUN_KEY"), "手机拿了凭据引用没用，还多漏一样");
    assert!(!raw.contains("mimeAllowlist"), "run-kit 的模型形状里没有这个键");
}

/// 覆盖参数：`?providerId=&modelId=` 只在**库里真存在且启用**时才认，否则 400 且不静默回落。
#[tokio::test]
async fn run_kit_rejects_bad_provider_or_model_overrides() {
    let state = state();
    seed_run_kit(&state);

    let (status, body) = get_json(
        router_for(state.clone()),
        "/api/ai/run-kit?providerId=p-run&modelId=m-bare",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert!(body["systemPrompt"].as_str().unwrap().contains("- Model: m-bare"));

    // Provider 不存在 / 没启用 → 400 CONFIG_ERROR（**不是 404**：前端按 error 分支、按 detail 给人看）
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/run-kit?providerId=nope").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert!(body["detail"].as_str().unwrap().contains("Provider 不存在或未启用: nope"));

    let (status, body) = get_json(
        router_for(state.clone()),
        "/api/ai/run-kit?providerId=p-run&modelId=nope",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert!(body["detail"].as_str().unwrap().contains("模型不存在或未启用: p-run/nope"));

    // 被用户关掉的模型同样不许指定（"指定了就按它跑"，不许悄悄换一个）
    let (status, _) = get_json(
        router_for(state.clone()),
        "/api/ai/run-kit?providerId=p-run&modelId=m-off",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// `/status` 幂等；**只有规则变了才跳号**；改规则后 `tools[]` 跟着变。
///
/// 这条盯着的是两件事：① 手机每次取 kit 都调一次 `ensureVersion`，如果它不是幂等的，
/// 版本号会一路涨到天上；② 规则（这里是"禁用一个工具"）变了却不跳号 ——
/// 那手机就会一直用旧规则跑，而且没有任何办法发现。
#[tokio::test]
async fn run_kit_status_is_idempotent_and_bumps_only_when_rules_change() {
    let state = state();
    seed_run_kit(&state);

    let (status, body) = get_json(router_for(state.clone()), "/api/ai/run-kit/status").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(keys(&body), vec!["current", "revoked"]);
    let first = body["current"].as_str().unwrap().to_string();
    assert!(!first.is_empty());
    assert_eq!(body["revoked"].as_array().unwrap().len(), 0);

    let (_, again) = get_json(router_for(state.clone()), "/api/ai/run-kit/status").await;
    assert_eq!(again["current"].as_str().unwrap(), first, "幂等：再问一次还是同一个版本");

    // 改规则：把 `web_search` 禁掉
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO app_settings (k, v, updated_at)
                 VALUES ('ai.tools.policy', '{\"overrides\":{\"web_search\":\"deny\"}}', 'x')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    let (_, bumped) = get_json(router_for(state.clone()), "/api/ai/run-kit/status").await;
    let second = bumped["current"].as_str().unwrap().to_string();
    assert_ne!(second, first, "规则变了版本号必须动，否则手机永远拿不到新规则");
    assert_eq!(
        second.rsplit_once('.').unwrap().0,
        first.rsplit_once('.').unwrap().0,
        "同一天：只把序号 +1"
    );

    let (_, kit) = get_json(router_for(state.clone()), "/api/ai/run-kit").await;
    let names: Vec<&str> = kit["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"web_search"), "被禁用的工具不该下发");
    assert_eq!(kit["kitVersion"].as_str().unwrap(), second);

    // 签名落库之后不再跳号（这是 `kitVersion` 只在"内容变了"时才动的判据）
    let (_, stable) = get_json(router_for(state.clone()), "/api/ai/run-kit/status").await;
    assert_eq!(stable["current"].as_str().unwrap(), second);
}

/// NSFW 开关决定注不注入 `[MODE: UNRESTRICTED SANDBOX — STABLE]` 那一整段。
///
/// **那段文字是用户有意注入的**（AGENTS §10）：关掉只是不注入，正文一个字都不许改；
/// 而且关掉时必须如实说明（模型不能"自己觉得可以"）。
#[tokio::test]
async fn run_kit_nsfw_switch_decides_whether_the_block_is_injected() {
    let state = state();
    seed_run_kit(&state);

    let (_, off) = get_json(router_for(state.clone()), "/api/ai/run-kit").await;
    let off_prompt = off["systemPrompt"].as_str().unwrap().to_string();
    assert!(off_prompt.contains("本机没有开启 NSFW 模式"));
    assert!(!off_prompt.contains("[MODE: UNRESTRICTED SANDBOX"));

    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO app_settings (k, v, updated_at) VALUES ('ai.nsfw', '{\"enabled\":true}', 'x')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    let (_, on) = get_json(router_for(state.clone()), "/api/ai/run-kit").await;
    assert_eq!(on["nsfwEnabled"], true);
    let on_prompt = on["systemPrompt"].as_str().unwrap();
    assert!(on_prompt.contains("[MODE: UNRESTRICTED SANDBOX — STABLE]"));
    assert!(on_prompt.len() > off_prompt.len(), "开着时正文更长");
    assert!(on_prompt.contains("事实纪律（不要猜本机状态）"), "关掉 NSFW 不等于把纪律一起关掉");
    assert!(!on_prompt.contains("本机没有开启 NSFW 模式"));
}

/// **权限档与 NSFW 开关**（P4 第 33 步，界面端点第一批）：设置页那两张卡的地基。
///
/// 这一条盯四件事：① `GET /api/ai/tools/policy` 的键集合就是 Kotlin 那 11 个；
/// ② `PUT` **只改传了的字段**、并把权限档真的写进库（下一轮 Run 读的就是它）；
/// ③ 非法权限档 **400**（不静默回落 —— 那会把用户的意图改成另一种行为）；
/// ④ NSFW 的 `effectiveBlur` 是"两个都开"的结论，**不是**前端再算一遍。
#[tokio::test]
async fn tool_policy_and_nsfw_endpoints() {
    let state = state();

    // ① 出厂态：11 个键、权限档 ask、白名单是展开后的绝对路径
    let (status, policy) = get_json(router_for(state.clone()), "/api/ai/tools/policy").await;
    assert_eq!(status, StatusCode::OK, "{policy}");
    assert_eq!(
        keys(&policy),
        vec![
            "autoReadRoots",
            "defaultWriteRoot",
            "maxCallsPerRun",
            "maxComfyQueriesPerRun",
            "maxReadBytes",
            "maxToolSteps",
            "maxWriteBytes",
            "overrides",
            "permissionMode",
            "readRoots",
            "writeRoots",
        ]
    );
    assert_eq!(policy["permissionMode"], "ask");
    assert_eq!(policy["maxToolSteps"], 25);
    assert_eq!(policy["maxCallsPerRun"], 48);
    assert!(
        policy["writeRoots"][0].as_str().unwrap().ends_with("comfyui"),
        "回的是展开后的绝对路径：{policy}"
    );

    // ② 切到「自动允许（无需批准）」→ 写进库、回读就是 full
    let (status, saved) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/tools/policy",
        serde_json::json!({ "permissionMode": "full" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["permissionMode"], "full");
    assert_eq!(saved["writeRoots"], policy["writeRoots"], "没传的字段照旧");
    let stored: String = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT v FROM app_settings WHERE k = 'ai.tools.policy'",
                [],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    assert!(stored.contains("\"permissionMode\":\"full\""), "{stored}");

    // ③ 非法权限档 → 400 且 `detail` 是人话（AI 错误那一档）
    let (status, err) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/tools/policy",
        serde_json::json!({ "permissionMode": "whatever" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"], "CONFIG_ERROR");
    assert!(err["detail"].as_str().unwrap().contains("未知的权限档"), "{err}");
    // 真的没被改动（"拒绝"不能变成"悄悄按 ask 存下来"）
    let (_, after) = get_json(router_for(state.clone()), "/api/ai/tools/policy").await;
    assert_eq!(after["permissionMode"], "full", "被拒的请求不许改库");
    // 大小写与空白按 Kotlin 的口径**先 trim + lowercase**（"FULL " 是合法的 full，
    // 存进去的是规范化之后的值 —— 库里别留一个带空格的权限档）
    let (status, saved) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/tools/policy",
        serde_json::json!({ "permissionMode": " FULL " }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["permissionMode"], "full");

    // ④ 工具清单：只列这台后端能跑的（40 个），六个键与 `AiToolInfo` 对齐
    let (status, tools) = get_json(router_for(state.clone()), "/api/ai/tools").await;
    assert_eq!(status, StatusCode::OK);
    let tools = tools.as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    // ⚠ 第 40 步起桌面文件工具五件也在这份清单里（在此之前只看手机清单，
    // 实现了界面也看不见 —— 用户会以为"这台后端不能改文件"）
    assert_eq!(
        names,
        vec![
            "ask_user_question",
            // 联网三件（P4 第 64 步）：Kotlin 给它们写的是 `BOTH`，所以它们住在
            // `mobile_tools.rs`（机器抽取的那份），但**实现**是这一步才落地的
            "civitai_search",
            // 审核（P4 第 63 步）：七项检查只报事实，"必跑不了"的三项才进提交闸门
            "comfy_audit_workflow",
            // 配方三件（P4 第 67 步）：学（会往本机配方库写一行）/ 列 / 绑（**只报不改**）。
            // 学与绑都与 `POST /api/ai/workflow-recipes*` 共用一份实现 —— 按 name 升序排在这里
            "comfy_bind_recipe",
            // 体检两件（P4 第 62 步）：列"还等着外部给输入的位置" / 跟本机模型清单对账，
            // 都**只报不改**（用户建议 ①/⑤）—— 按 name 升序排在这里
            "comfy_check_inputs",
            // 接线体检（2026-10-17 用户"对于错误接线要能向用户指出"）：只读、出厂 allow ——
            // 与 `comfy_audit_workflow` 分工不同（那条只审 API 图、看"跑起来会不会崩"）
            "comfy_diagnose_workflow",
            // 本机模型清单对账（O3）：只读，**不属于 COMFY 那一类**（不吃查询预算）
            "comfy_env_report",
            // 两段式找工作流（用户 bug ⑤）：库里搜不到就回头看本机的工作流文件
            "comfy_find_workflow",
            "comfy_fit_workflow",
            "comfy_get_run",
            "comfy_get_status",
            "comfy_learn_workflow",
            "comfy_list_recipes",
            "comfy_list_workflows",
            "comfy_load_workflow",
            // 图像编辑三件（投放 + 遮罩）：两阶段编辑的地基，出厂档都是 ask
            "comfy_make_mask",
            // 摘节点 / 拼节点（P4 第 63 步，用户建议 ②）：改好的图是**内联**返回的
            "comfy_prune_workflow",
            // 起 / 停实例（2026-09-28 用户 bug「通过 CLI 启动 comfyui 总是失败」）：
            // ComfyHub 自己用工作区的 `.venv\python.exe` 起 —— comfy-cli 那条路要写
            // `%LOCALAPPDATA%\comfy-cli\config.ini` 与 `<ComfyUI>\user\comfyui_8188.log`，
            // 受限环境里必然 Errno 13。两件出厂档都是 ask（起/杀一个占显存的进程）
            "comfy_start",
            "comfy_stop",
            "comfy_submit",
            "comfy_sync_history",
            "comfy_use_attachment",
            "comfy_use_media",
            // danbooru（会把数据送出本机的工具；出厂档 allow，但受联网预算约束）
            "danbooru_lookup",
            "delete_skill",
            // 定向查询（P4 第 64 步）：先查本机、再按 Civitai → HF → ModelScope 查，
            // **绝不代下**（只给链接 + sha256 + 目标目录）
            "find_model",
            "list_dir",
            "list_skills",
            "load_skill",
            "read_file",
            "register_skill",
            "remember",
            // 第 46 步：资料库（写角色之前先查本机，而不是凭印象编 tag）
            "save_character_card",
            // 画风卡（2026-10-21）：与角色卡同等地位
            "save_style_card",
            "search_character_cards",
            "search_prompts",
            "search_style_cards",
            "web_fetch",
            "web_search",
            "write_file",
        ]
    );
    assert_eq!(
        keys(&tools[0]),
        vec!["access", "category", "description", "mutating", "name", "overridden"]
    );
    assert_eq!(tools[0]["category"], "ask");
    assert_eq!(tools[0]["access"], "allow");
    let by_name = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap();
    assert_eq!(by_name("remember")["category"], "memory");
    assert_eq!(by_name("remember")["mutating"], true, "`remember` 会写盘");
    assert_eq!(by_name("read_file")["category"], "files");
    assert_eq!(by_name("write_file")["mutating"], true, "`write_file` 会写盘");
    // 这一次策略已经被前面切成了 `full`，所以 `delete_skill` 的出厂档 ASK 被放宽成 allow
    // （出厂档本身在 `tools::tests::mutating_flags_come_from_the_generated_catalog` 里钉住）
    assert_eq!(by_name("delete_skill")["access"], "allow", "full 档下 ask → allow");
    assert_eq!(tools[0]["overridden"], false);
    // 覆盖过之后界面要能标出来
    let (_, saved) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/tools/policy",
        serde_json::json!({ "overrides": { "remember": "deny" } }),
    )
    .await;
    assert_eq!(saved["permissionMode"], "full", "没传权限档 → 照旧");
    assert_eq!(saved["overrides"]["remember"], "deny");
    let (_, tools) = get_json(router_for(state.clone()), "/api/ai/tools").await;
    let remember = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "remember")
        .unwrap()
        .clone();
    assert_eq!(remember["access"], "deny");
    assert_eq!(remember["overridden"], true);

    // ⑤ NSFW：出厂两档都关、版本号在；PUT 之后 effectiveBlur 才是 true
    let (status, nsfw) = get_json(router_for(state.clone()), "/api/ai/nsfw").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&nsfw), vec!["blur", "effectiveBlur", "enabled", "promptVersion"]);
    assert_eq!(nsfw["enabled"], false);
    assert_eq!(nsfw["effectiveBlur"], false);
    assert!(nsfw["promptVersion"].as_str().unwrap().starts_with('v'), "{nsfw}");

    // 只开 enabled（不传 blur）→ blur 按**关**处理（漏传不能让打码被悄悄打开）
    let (_, saved) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/nsfw",
        serde_json::json!({ "enabled": true }),
    )
    .await;
    assert_eq!(saved["enabled"], true);
    assert_eq!(saved["blur"], false);
    assert_eq!(saved["effectiveBlur"], false, "两个都开才生效");
    // 两档都开 → effectiveBlur
    let (_, saved) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/nsfw",
        serde_json::json!({ "enabled": true, "blur": true }),
    )
    .await;
    assert_eq!(saved["effectiveBlur"], true);
    // run-kit 读的是**同一行**（各写一份键名就会出现"界面开了、提示词没注入"）
    assert_eq!(comfyhub_server_rs::ai::run_kit::nsfw_enabled(&state).unwrap(), true);
}

/// `GET /skills/roots` 报的两个路径必须**就是**读写实际用的那两个（拼错一处就是
/// "界面让你往 A 拷、程序去 B 找"，而这种错不报任何错）。
#[tokio::test]
async fn skills_roots_points_at_the_directories_the_endpoints_really_use() {
    let state = state();
    let (builtin, user) = skill_dirs(&state);

    let (status, body) = get_json(router_for(state.clone()), "/api/ai/skills/roots").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["builtinRoot", "userRoot", "userRootExists"]);
    assert_eq!(body["userRoot"], user.to_string_lossy().to_string());
    assert_eq!(body["builtinRoot"], builtin.to_string_lossy().to_string());
    assert_eq!(body["userRootExists"], false, "还没建过投放口");

    // 写一个进去之后 `userRootExists` 要翻成 true（**现场算**，不是缓存）
    let (status, _) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/skills",
        serde_json::json!({ "name": "demo", "description": "演示", "content": "正文" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, body) = get_json(router_for(state.clone()), "/api/ai/skills/roots").await;
    assert_eq!(body["userRootExists"], true);
}

/// 注册 → 列表 → 详情 → 删除的完整一轮，以及三档失败（非法名 / 缺字段 / 不存在的名字）。
///
/// 盯的是**形状与稳定错误码**：`{error: 稳定码, detail: 人话}` 的码写错，界面就没法按码
/// 分支（"这个名字不能用"与"内置的不能删"是两种完全不同的处置）。
#[tokio::test]
async fn skills_crud_round_trip_with_stable_error_codes() {
    let state = state();
    let (_, user_root) = skill_dirs(&state);

    // 空清单（没有内置目录、投放口也还没建）
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/skills").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!([]));

    // 注册：201 + **15 个键**（可空的那 7 个一个都不许自己冒出来）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/skills",
        serde_json::json!({
            "name": "demo-skill",
            "description": "演示用",
            "whenToUse": "用户说'演示'时",
            "content": "正文第一行\n正文第二行"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        keys(&body),
        vec![
            "description", "digest", "enabled", "fileCount", "modelInvocable", "name", "sizeBytes",
            "source", "updatedAt", "userInvocable", "version", "whenToUse"
        ],
        "version: 1 是 render 写死的 → 会读回来；conflict / validationError / origin 不该出现"
    );
    assert_eq!(body["name"], "demo-skill");
    assert_eq!(body["source"], "user");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["userInvocable"], true);
    assert_eq!(body["modelInvocable"], true);
    assert_eq!(body["version"], "1");
    assert_eq!(body["fileCount"], 1);
    assert!(body["sizeBytes"].as_i64().unwrap() > 0);
    assert_eq!(body["digest"].as_str().unwrap().len(), 16, "sha256 前 16 位");

    // 落盘的格式（render 逐字）：frontmatter + 空行 + 正文 + 结尾换行
    let file = user_root.join("demo-skill").join("SKILL.md");
    let raw = std::fs::read_to_string(&file).expect("文件应当写出来了");
    assert_eq!(
        raw,
        "---\nname: demo-skill\ndescription: 演示用\nwhenToUse: 用户说'演示'时\nversion: 1\n---\n\n正文第一行\n正文第二行\n"
    );

    // 列表里就一条
    let (_, body) = get_json(router_for(state.clone()), "/api/ai/skills").await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["name"], "demo-skill");

    // 详情：元数据 + **去 frontmatter 的正文**
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/skills/demo-skill").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["content", "skill"]);
    assert_eq!(body["content"], "正文第一行\n正文第二行");
    assert_eq!(body["skill"]["name"], "demo-skill");

    // 不存在的名字 → 400 CONFIG_ERROR（**不是 404**：Dart 按 detail 给人看）
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/skills/nope").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert_eq!(body["detail"], "没有名为 nope 的 skill");

    // 非法名 → 稳定码 INVALID_SKILL_NAME
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/skills",
        serde_json::json!({ "name": "Bad_Name", "description": "x", "content": "y" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "INVALID_SKILL_NAME");
    assert_eq!(
        body["detail"],
        "非法 skill 名「Bad_Name」：只允许小写字母 / 数字 / 连字符，且以字母或数字开头（≤64 字符）"
    );

    // 缺 description / 空正文 → INVALID_ARGUMENT（两条不同的文案）
    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/skills",
        serde_json::json!({ "name": "demo2", "description": "   ", "content": "y" }),
    )
    .await;
    assert_eq!(body["error"], "INVALID_ARGUMENT");
    assert_eq!(body["detail"], "description 必填：说明这个 skill 什么时候用");
    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/skills",
        serde_json::json!({ "name": "demo2", "description": "x", "content": "   " }),
    )
    .await;
    assert_eq!(body["detail"], "skill 正文不能为空");

    // 删除 → `{deleted:true, id}`；再删一次是 `{deleted:false}`（**不是 404**，幂等）
    let (status, body) = send_empty(router_for(state.clone()), "DELETE", "/api/ai/skills/demo-skill").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({ "deleted": true, "id": "demo-skill" }));
    assert!(!file.exists(), "bundle 目录要整个删掉（含 references/）");
    let (_, body) = send_empty(router_for(state.clone()), "DELETE", "/api/ai/skills/demo-skill").await;
    assert_eq!(body, serde_json::json!({ "deleted": false, "id": "demo-skill" }));
}

/// 内置 skill：**只能停用不能删除**；同名时用户版胜出并带上 `conflict`（AIH-037）。
///
/// 这两条合起来才是"用户能覆盖内置的、但删不掉项目自带的那份"。
#[tokio::test]
async fn skills_builtin_is_readonly_and_user_override_wins_with_a_conflict_hint() {
    let state = state();
    let (builtin, user_root) = skill_dirs(&state);
    std::fs::create_dir_all(builtin.join("native")).unwrap();
    std::fs::write(
        builtin.join("native").join("SKILL.md"),
        "---\nname: native\ndescription: 内置版\n---\n内置正文",
    )
    .unwrap();

    let (_, body) = get_json(router_for(state.clone()), "/api/ai/skills").await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["source"], "builtin");
    assert!(body[0].get("conflict").is_none(), "没被覆盖时不该有 conflict 键");

    // 删内置 → SKILL_READONLY（且文件还在）
    let (status, body) = send_empty(router_for(state.clone()), "DELETE", "/api/ai/skills/native").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "SKILL_READONLY");
    assert_eq!(body["detail"], "「native」是内置 skill，只能停用不能删除");
    assert!(builtin.join("native").join("SKILL.md").exists());

    // 用户在投放口放同名 → 用户版胜出，带 conflict 提示，且 source = user
    std::fs::create_dir_all(user_root.join("native")).unwrap();
    std::fs::write(
        user_root.join("native").join("SKILL.md"),
        "---\nname: native\ndescription: 用户版\n---\n用户正文",
    )
    .unwrap();
    let (_, body) = get_json(router_for(state.clone()), "/api/ai/skills").await;
    assert_eq!(body.as_array().unwrap().len(), 1, "同名只留一条");
    assert_eq!(body[0]["source"], "user");
    assert_eq!(body[0]["description"], "用户版");
    assert_eq!(body[0]["conflict"], "覆盖了同名内置 skill");

    // 详情读到的也是用户那份正文
    let (_, body) = get_json(router_for(state.clone()), "/api/ai/skills/native").await;
    assert_eq!(body["content"], "用户正文");

    // 非法项（目录名与 frontmatter 的 name 不一致）照样列出来 + 带 validationError，
    // 但**详情读不了**（SKILL_INVALID）—— AIH-038 的两半
    std::fs::create_dir_all(user_root.join("broken")).unwrap();
    std::fs::write(
        user_root.join("broken").join("SKILL.md"),
        "---\nname: other-name\ndescription: x\n---\n正文",
    )
    .unwrap();
    let (_, body) = get_json(router_for(state.clone()), "/api/ai/skills").await;
    let broken = body
        .as_array()
        .unwrap()
        .iter()
        .find(|dto| dto["name"] == "other-name")
        .expect("非法项也要列出来");
    assert_eq!(
        broken["validationError"],
        "frontmatter 的 name「other-name」与目录名「broken」不一致"
    );
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/skills/other-name").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "SKILL_INVALID");
    assert_eq!(
        body["detail"],
        "skill「other-name」不合法，不能加载：frontmatter 的 name「other-name」与目录名「broken」不一致"
    );
}

/// 投放口自动登记（`POST /skills/rescan`）：**只补没有 frontmatter 的文件**，
/// 已经有 frontmatter 的一个字节都不许改（那是用户自己写的内容）。
#[tokio::test]
async fn skills_rescan_registers_plain_files_and_never_touches_frontmatter() {
    let state = state();
    let (_, user_root) = skill_dirs(&state);
    std::fs::create_dir_all(&user_root).unwrap();

    // ① 平铺 `.md`，没有 frontmatter → 补 name（文件名兜底）+ description（正文第一行）
    std::fs::write(user_root.join("flat-one.md"), "# 大标题\n这是正文。\n第二行").unwrap();
    // ② 已经有 frontmatter（而且**不合法**：缺 description）→ 一个字节都不许改
    let already = "---\nname: already-there\n---\n正文\n";
    std::fs::write(user_root.join("already-there.md"), already).unwrap();
    // ③ 中文文件名 → 收敛不出 kebab-case，如实报错（别静默跳过）
    std::fs::write(user_root.join("中文名.md"), "正文").unwrap();
    // ④ 空正文
    std::fs::write(user_root.join("empty-one.md"), "   \n").unwrap();

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/skills/rescan",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["errors", "names", "registered", "skills"]);
    assert_eq!(body["registered"], 1);
    assert_eq!(body["names"], serde_json::json!(["flat-one"]));
    let errors: Vec<String> = body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(errors.len(), 2, "两件放进来也没法用的东西要**都**说出来：{errors:?}");
    assert!(errors.iter().any(|e| e.contains("中文名.md") && e.contains("kebab-case")), "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("empty-one.md") && e.contains("正文为空")), "{errors:?}");

    // 补出来的 frontmatter：name 来自文件名、description 来自正文第一行（去掉了 `#`）
    let raw = std::fs::read_to_string(user_root.join("flat-one.md")).unwrap();
    assert_eq!(
        raw,
        "---\nname: flat-one\ndescription: 大标题\nversion: 1\n---\n\n# 大标题\n这是正文。\n第二行\n"
    );
    // 已经有 frontmatter 的**一个字节都没变**
    assert_eq!(std::fs::read_to_string(user_root.join("already-there.md")).unwrap(), already);

    // rescan 的 `skills` 是**扫描后的完整清单**（含那条非法的 already-there）
    let names: Vec<&str> = body["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|dto| dto["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"flat-one"), "{names:?}");
    assert!(names.contains(&"already-there"), "{names:?}");

    // 再跑一次：没有可登记的了（幂等 —— `flat-one` 现在已经有 frontmatter）
    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/skills/rescan",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(body["registered"], 0);
}

/// 读 / 整篇替换 / 加一条 / 删几条 / 清空的完整一轮。
///
/// 三条纪律各有一条断言：**超限报错（不静默截断）/ 用户手敲的不加日期前缀 /
/// 越界下标忽略掉（不整批失败）**。
#[tokio::test]
async fn memory_endpoints_round_trip_and_report_stable_codes() {
    let state = state();
    let dir = memory_dir(&state);

    // 初始：空 + 两个上限（界面显示 "N / 100 条" 用）
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/memory").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["content", "entryCount", "maxChars", "maxEntries", "path", "updatedAt"]);
    assert_eq!(body["content"], "");
    assert_eq!(body["entryCount"], 0);
    assert_eq!(body["maxChars"], 8000);
    assert_eq!(body["maxEntries"], 100);
    assert!(body["path"].as_str().unwrap().ends_with("memory.md"));

    // 整篇替换
    let (status, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/memory",
        serde_json::json!({ "content": "- 甲\n- 乙\n" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["content"], "- 甲\n- 乙", "读回来去掉首尾空白（幂等往返）");
    assert_eq!(body["entryCount"], 2);
    assert_eq!(std::fs::read_to_string(dir.join("memory.md")).unwrap(), "- 甲\n- 乙\n");

    // 加一条：**用户手敲的不带日期前缀**（日期前缀只有 AI 的 remember 加）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/memory/entries",
        serde_json::json!({ "content": "丙" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["content"], "- 甲\n- 乙\n- 丙");
    assert!(!body["content"].as_str().unwrap().contains('['), "不许给用户的话加日期前缀");

    // 再说一遍不算新增（判重）
    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/memory/entries",
        serde_json::json!({ "content": "丙" }),
    )
    .await;
    assert_eq!(body["entryCount"], 3);

    // 按下标删：1 = 乙；99 越界 → **忽略**（界面手里的列表可能只差一瞬间）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/memory/delete",
        serde_json::json!({ "indices": [1, 99] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["content"], "- 甲\n- 丙");

    // 空下标 = 原样返回（不清空！）
    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/memory/delete",
        serde_json::json!({ "indices": [] }),
    )
    .await;
    assert_eq!(body["content"], "- 甲\n- 丙");

    // 超限：**报错而不是静默截断**（悄悄截断 = 悄悄丢用户的话）
    let too_long = "字".repeat(8001);
    let (status, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/memory",
        serde_json::json!({ "content": too_long }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "MEMORY_TOO_LARGE");
    assert_eq!(body["detail"], "记忆太长（8001 > 8000 字符），请删掉一些再保存");
    // 失败之后**原内容还在**（没有半截写坏）
    let (_, body) = get_json(router_for(state.clone()), "/api/ai/memory").await;
    assert_eq!(body["content"], "- 甲\n- 丙");

    // 一条太长 → INVALID_ARGUMENT（**与上一档不同的码**：一个该拆短、一个该去清理）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/memory/entries",
        serde_json::json!({ "content": "字".repeat(501) }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "INVALID_ARGUMENT");
    assert_eq!(body["detail"], "一条记忆太长（501 > 500 字符），拆成几条");

    // 空内容 → INVALID_ARGUMENT
    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/memory/entries",
        serde_json::json!({ "content": "   " }),
    )
    .await;
    assert_eq!(body["error"], "INVALID_ARGUMENT");
    assert_eq!(body["detail"], "记的内容不能为空");

    // 清空
    let (status, body) = send_empty(router_for(state.clone()), "DELETE", "/api/ai/memory").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["content"], "");
    assert_eq!(body["entryCount"], 0);
    assert_eq!(std::fs::read_to_string(dir.join("memory.md")).unwrap(), "");
}

/// 长期记忆**也进系统提示与 run-kit**：写完立刻在 `run-kit` 里读得到（不做缓存）。
///
/// 这条把"界面写的"与"模型看到的"接上 —— 少这一环，用户会以为"我写了它却记不住"。
#[tokio::test]
async fn memory_written_from_the_ui_shows_up_in_the_run_kit_prompt() {
    let state = state();
    add_provider(&state, "p-kit", Some("KIT_KEY"));
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_models (provider_id, model_id, display_name, enabled, tools, reasoning)
                 VALUES ('p-kit', 'm-1', '夹具模型', 1, 1, 0)",
                [],
            )?;
            Ok(())
        })
        .expect("插模型");
    let (_, _) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/memory",
        serde_json::json!({ "content": "- 用户喜欢国风" }),
    )
    .await;

    let (status, kit) = get_json(router_for(state.clone()), "/api/ai/run-kit?providerId=p-kit&modelId=m-1").await;
    assert_eq!(status, StatusCode::OK);
    let prompt = kit["systemPrompt"].as_str().unwrap();
    assert!(prompt.contains("用户喜欢国风"), "刚写的记忆必须立刻进提示词（刻意不缓存）");
}

/// Skills 两个写侧工具：注册 → 立刻出现在列表里 → 删除；再删报 `SKILL_NOT_FOUND`。
#[tokio::test]
async fn skill_tools_register_then_delete() {
    let state = state();
    let storage = state.media_dir().parent().unwrap().to_path_buf();

    let out = call_tool(
        &state,
        "register_skill",
        serde_json::json!({
            "name": "e2e-helper",
            "description": "给第 40 步的用例用",
            "whenToUse": "只在用例里",
            "content": "## 步骤\n1. 别把它当真的规则"
        }),
    )
    .await
    .unwrap();
    assert!(out.content.contains("已注册 skill「e2e-helper」"), "{}", out.content);
    // 结构化负载是完整的 SkillDto（界面按它画卡片）
    assert_eq!(out.payload["name"], "e2e-helper");
    assert_eq!(out.payload["source"], "user");
    assert!(out.payload["digest"].is_string(), "{:?}", out.payload);
    // 磁盘是真源：文件真的在
    assert!(storage
        .join("ai")
        .join("skills")
        .join("e2e-helper")
        .join("SKILL.md")
        .is_file());

    // 同一个名字再注册一次 = 覆盖（不报错，与 Kotlin 同）
    let again = call_tool(
        &state,
        "register_skill",
        serde_json::json!({ "name": "e2e-helper", "description": "覆盖一版", "content": "新正文" }),
    )
    .await
    .unwrap();
    assert!(again.content.contains("已注册"), "{}", again.content);

    let deleted = call_tool(&state, "delete_skill", serde_json::json!({ "name": "e2e-helper" }))
        .await
        .unwrap();
    assert_eq!(deleted.payload["deleted"], true);
    assert!(!storage.join("ai").join("skills").join("e2e-helper").exists());

    let missing = call_tool(&state, "delete_skill", serde_json::json!({ "name": "e2e-helper" }))
        .await
        .unwrap_err();
    assert_eq!(missing.code, "SKILL_NOT_FOUND");

    // 内置 skill 受保护（码原样透传，别改写成 TOOL_ERROR）
    let builtin = call_tool(&state, "delete_skill", serde_json::json!({ "name": "no-such-builtin" }))
        .await
        .unwrap_err();
    assert_eq!(builtin.code, "SKILL_NOT_FOUND");

    std::fs::remove_dir_all(state.project_root()).ok();
}
