//! `tools` 的单元测试（主体代码在 [`tools`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

/// T1：`comfy_recent_successes` 必须是**已实现、只读、出厂 ALLOW** 的桌面工具
/// （只读工具出厂即放行，不需要为每次"查询上次成功"确认）。
#[test]
fn comfy_recent_successes_是只读的_allow_工具() {
    assert!(IMPLEMENTED.contains(&"comfy_recent_successes"));
    assert_eq!(default_access("comfy_recent_successes"), "ALLOW");
    assert!(!is_mutating("comfy_recent_successes"));
    assert!(
        crate::tools::desktop_tools::DESKTOP_TOOLS
            .iter()
            .any(|tool| tool.name == "comfy_recent_successes"),
        "目录里必须有它，否则 implemented() 收不到"
    );
}

/// 读侧 `recall_memory`（2026-11-14）：只读、出厂 ALLOW —— 查自己记过什么不该先点批准。
#[test]
fn recall_memory_是只读的_allow_工具() {
    assert!(IMPLEMENTED.contains(&"recall_memory"));
    assert_eq!(default_access("recall_memory"), "ALLOW");
    assert!(!is_mutating("recall_memory"));
    assert!(
        crate::tools::desktop_tools::DESKTOP_TOOLS
            .iter()
            .any(|tool| tool.name == "recall_memory"),
        "目录里必须有它，否则 implemented() 收不到"
    );
}

/// `recall_memory`：系统提示只放最新一截，它是"取回剩下那一半"的唯一入口。
///
/// 判据四条：① 关键词检索（多词取交集）；② 从最新往回翻页 + 翻到头要说清楚；
/// ③ 无命中说人话、不许空着；④ 顺序号与界面「长期记忆」列表的行序一致。
#[test]
fn recall_memory_按关键词与翻页取回被省略的条目() {
    use crate::web::routes::AppState;

    let dir = crate::infra::testpaths::scratch("recall-memory");
    let memory_dir = dir.join("ai");
    std::fs::create_dir_all(&memory_dir).unwrap();
    std::fs::create_dir_all(dir.join("media")).unwrap();
    // 30 条：偶数条是"画风校准"，第 27 条起多一个"光影"
    let lines: Vec<String> = (1..=30)
        .map(|i| {
            let tag = if i % 2 == 0 {
                "画风校准"
            } else {
                "工作流笔记"
            };
            let extra = if i >= 27 { " 光影" } else { "" };
            format!("- [2026-11-14] 第{i:02}条 {tag}{extra}")
        })
        .collect();
    std::fs::write(memory_dir.join("memory.md"), lines.join("\n") + "\n").unwrap();

    let state = AppState::new(
        crate::infra::db::open_memory().unwrap(),
        ":memory:",
        dir.join("media"),
    );

    // 关键词：只回命中的（新的在前），匹配数与顺序号都是真的
    let out = recall_memory(&state, &json!({ "query": "画风" })).unwrap();
    assert_eq!(out.payload["totalEntries"].as_i64(), Some(30));
    assert_eq!(out.payload["matched"].as_i64(), Some(15));
    let first = &out.payload["entries"][0];
    assert_eq!(first["index"].as_i64(), Some(30), "新的在前");
    assert!(first["text"].as_str().unwrap().contains("第30条"));
    assert!(
        !out.content.contains("第29条"),
        "没命中的条目不该混进来：{}",
        out.content
    );
    assert!(out.content.contains("匹配「画风」的有 15 条"));

    // 多词 = 交集（都命中才算）
    let out = recall_memory(&state, &json!({ "query": "画风 光影" })).unwrap();
    assert_eq!(out.payload["matched"].as_i64(), Some(2), "第 28 / 30 条");

    // 无命中：说人话，不许空着
    let out = recall_memory(&state, &json!({ "query": "不存在的词" })).unwrap();
    assert_eq!(out.payload["matched"].as_i64(), Some(0));
    assert!(
        out.content.contains("没有匹配「不存在的词」的记忆"),
        "{}",
        out.content
    );

    // 不带 query：从最新往回翻；offset 接着上一页；到头要说明
    let out = recall_memory(&state, &json!({ "limit": 10 })).unwrap();
    assert_eq!(out.payload["returned"].as_i64(), Some(10));
    assert_eq!(out.payload["hasMore"].as_bool(), Some(true));
    assert_eq!(out.payload["entries"][0]["index"].as_i64(), Some(30));
    assert!(out.content.contains("第 1–10 条"), "{}", out.content);
    let out = recall_memory(&state, &json!({ "limit": 10, "offset": 20 })).unwrap();
    assert_eq!(out.payload["returned"].as_i64(), Some(10));
    assert_eq!(out.payload["hasMore"].as_bool(), Some(false));
    assert!(out.content.contains("这已经是全部了"), "{}", out.content);
    let out = recall_memory(&state, &json!({ "offset": 30 })).unwrap();
    assert_eq!(out.payload["returned"].as_i64(), Some(0));
    assert!(out.content.contains("已经翻到头了"), "{}", out.content);

    // 一次最多 30 条（别让一次调用把整篇 5 万字全灌回上下文）
    let out = recall_memory(&state, &json!({ "limit": 500 })).unwrap();
    assert_eq!(out.payload["returned"].as_i64(), Some(30));

    // 空记忆：如实说，并给下一步
    std::fs::write(memory_dir.join("memory.md"), "").unwrap();
    let out = recall_memory(&state, &json!({})).unwrap();
    assert!(out.content.contains("还是空的"), "{}", out.content);
    assert_eq!(out.payload["totalEntries"].as_i64(), Some(0));

    std::fs::remove_dir_all(dir).ok();
}

/// 看卡片示例图（2026-11-13）必须是**已实现、只读、出厂 ALLOW** 的桌面工具：
/// 它只读本机库、把缩图交给执行器；看一张示例不该让用户先点一次批准。
#[test]
fn view_card_examples_是只读的_allow_工具() {
    assert!(IMPLEMENTED.contains(&"view_card_examples"));
    assert_eq!(default_access("view_card_examples"), "ALLOW");
    assert!(!is_mutating("view_card_examples"));
    assert!(
        crate::tools::desktop_tools::DESKTOP_TOOLS
            .iter()
            .any(|tool| tool.name == "view_card_examples"),
        "目录里必须有它，否则 implemented() 收不到"
    );
}

#[test]
fn specs_only_include_implemented_tools() {
    let policy = crate::tools::policy::ToolPolicy::new(
        crate::infra::testpaths::scratch("policy-root"),
        crate::tools::policy::ToolPolicyConfig::default(),
    );
    let specs = specs(&policy);
    for spec in &specs {
        assert!(
            IMPLEMENTED.contains(&spec.name.as_str()),
            "{} 没实现却下发了 —— 模型会去调一个必然失败的调用",
            spec.name
        );
        assert!(!spec.description.is_empty());
        assert!(
            spec.parameters.is_object(),
            "{} 的 schema 必须是对象",
            spec.name
        );
        // 工具卡需显示"为什么调用"。schema 中没有该可选字段，
        // 模型就算想写也会被 `additionalProperties:false` 拒掉。
        assert_eq!(
            spec.parameters["properties"]["reason"]["type"], "string",
            "{} 的 schema 少了 reason（工具卡只能显示机器名）",
            spec.name
        );
    }
    assert!(specs.iter().any(|s| s.name == "list_skills"));
    // 看卡片示例图（2026-11-13）：要与卡片查询一起下发，模型才知道能看画面。
    assert!(specs.iter().any(|s| s.name == "view_card_examples"));
    assert!(
        specs.iter().any(|s| s.name == "ask_user_question"),
        "提问闸门已搬，应当下发"
    );
    // 参考图搜索（2026-11-04）：按 Danbooru tag 找真实图、下载成本机附件
    assert!(specs.iter().any(|s| s.name == "reference_image_search"));
    // 联网三件 + 定向查询（P4 第 64 步）：**应当**出现在清单里 ——
    // 系统提示第 17 条点名的那四个工具到这一步才真的下发给模型（在此之前那一节指向
    // 的是模型看不见的工具，等于一句空话）。
    for expected in ["web_search", "web_fetch", "civitai_search", "find_model"] {
        assert!(
            specs.iter().any(|s| s.name == expected),
            "{expected} 应当下发：{:?}",
            specs.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }
    // 配方三件（P4 第 67 步）已经实现 → 与桌面那两件一样，必须同时出现在提示词与请求里
    // （在此之前那两条 `absent` 断言是对的：没实现的一律不许下发）
    for expected in [
        "comfy_learn_workflow",
        "comfy_list_recipes",
        "comfy_bind_recipe",
    ] {
        assert!(
            specs.iter().any(|s| s.name == expected),
            "{expected} 应当下发"
        );
    }
    // 2026-10-21：带 TextGenerate 的工作流由 AI 自己写终稿，`positivePrompt` 必须下发给模型，
    // 否则 skill 里写的流程模型根本调不到。
    let submit = specs
        .iter()
        .find(|s| s.name == "comfy_submit")
        .expect("comfy_submit 应当下发");
    assert_eq!(
        submit.parameters["properties"]["positivePrompt"]["type"], "string",
        "comfy_submit 少了 positivePrompt"
    );
}

/// 联网额度是一条**独立的线**（Kotlin `ToolContext.webInWindow` +
/// `ToolPolicyConfig.DEFAULT_MAX_WEB_QUERIES_PER_RUN`）：只认联网类工具、
/// 只在**实际执行成功**之后计数、完成作答时归零。
///
/// 为什么单列一条用例：这份预算的唯一执行点在执行器里按**工具类别**判，
/// 而"哪些工具算联网"那份映射在 `ai_settings` —— 两处一漂移，
/// 系统提示里那句"一次回复最多联网 8 次"就变成空话，**且不会有任何用例变红**。
#[test]
fn 联网额度只认联网类工具且只在成功时计数() {
    let mut session = ToolSession::default();
    assert!(session.has_web_queries_left());

    // 非联网工具不占联网额度（否则一次"搜自己的库"就把联网预算吃掉了）
    session.spend_call("search_prompts");
    session.spend_call("comfy_get_status");
    assert_eq!(session.web_queries_in_window, 0);
    assert_eq!(session.calls_in_window, 2, "总调用额度照旧要记");

    session.spend_call("danbooru_lookup");
    assert_eq!(session.web_queries_in_window, 1);
    // `find_model` 也是 WEB 档（P4 第 64 步）—— 它同样吃这一份额度，
    // 所以"这条工具不吃预算"这种漂移会**静默**发生（`ai_settings::category_of`）
    session.spend_call("find_model");
    assert_eq!(session.web_queries_in_window, 2);

    // 吃到上限 → 预检就该拦（执行器回 QUERY_BUDGET_EXCEEDED）
    for _ in 2..crate::tools::policy::MAX_WEB_QUERIES_PER_RUN {
        session.spend_call("danbooru_lookup");
    }
    assert_eq!(
        session.web_queries_in_window,
        crate::tools::policy::MAX_WEB_QUERIES_PER_RUN
    );
    assert!(!session.has_web_queries_left());

    // 完成作答 → 联网额度也归零（与总调用额度同一档重置）
    session.reset_budget();
    assert!(session.has_web_queries_left());
    assert_eq!(session.web_queries_in_window, 0);
}

/// **命中缓存的那一次不占联网 / ComfyUI 额度**（`tools::tool_cache`）：
/// 那两条额度的语义是"别拿外部服务当轮询器"，而命中缓存时**一次外部请求都没发**。
///
/// 为什么单列一条：这条线一漂移，表现就是"明明没打扰人家，却被告知额度用尽"
/// （模型被限制、界面显示一句 `QUERY_BUDGET_EXCEEDED`），且**不会有其他用例失败**。
#[test]
fn 命中缓存的那一次不占联网与_comfy_额度() {
    let mut session = ToolSession::default();
    session.spend_cached_call("web_search");
    session.spend_cached_call("comfy_get_status");
    assert_eq!(session.web_queries_in_window, 0, "没发请求就不该占联网额度");
    assert_eq!(
        session.comfy_queries_in_window, 0,
        "同上：没问 ComfyUI 就不该占它的额度"
    );
    assert_eq!(
        session.calls, 2,
        "总调用额度照记：命中同样多一轮往返、同样烧上下文"
    );
    assert_eq!(session.calls_in_window, 2);

    // 与"真跑成功"那一档对照：同名的真跑是要占的
    session.spend_call("web_search");
    assert_eq!(session.web_queries_in_window, 1);
}

/// **ComfyUI 查询额度**是另一条独立的线（AIH-036「不许把 ComfyUI 当轮询器」，
/// Kotlin `ToolContext.comfyInWindow` + `DEFAULT_MAX_COMFY_QUERIES_PER_RUN = 9`）。
///
/// 为什么必须有这条用例：系统提示里写着"一次回复最多查 ComfyUI N 次"，
/// 而在 2026-09-25 那次审查里它**是空的**（Rust 执行器只拦了总调用数与联网数）——
/// "提示词里写了、代码里没有"这类缺口只有用例钉得住。
#[test]
fn comfy_查询额度只认_comfy_类工具且只在成功时计数() {
    let mut session = ToolSession::default();
    assert!(session.has_comfy_queries_left());

    // 其它类别不占这份额度（否则"读一个文件"就把 ComfyUI 预算吃掉了）
    session.spend_call("read_file");
    session.spend_call("search_prompts");
    session.spend_call("danbooru_lookup");
    session.spend_call("load_skill");
    assert_eq!(session.comfy_queries_in_window, 0);
    assert_eq!(session.calls_in_window, 4, "总调用额度照旧要记");

    // `comfy_*` 三件查询都算（它们常被用于"再次确认"）
    session.spend_call("comfy_get_status");
    session.spend_call("comfy_list_workflows");
    session.spend_call("comfy_get_run");
    assert_eq!(session.comfy_queries_in_window, 3);
    assert_eq!(
        session.web_queries_in_window, 1,
        "danbooru_lookup 走的是**联网**那份额度，两份额度互不串味"
    );

    for _ in 3..crate::tools::policy::MAX_COMFY_QUERIES_PER_RUN {
        session.spend_call("comfy_get_status");
    }
    assert_eq!(
        session.comfy_queries_in_window,
        crate::tools::policy::MAX_COMFY_QUERIES_PER_RUN
    );
    assert!(!session.has_comfy_queries_left(), "到上限后预检必须拦得住");
    assert!(
        session.has_web_queries_left(),
        "拦住 ComfyUI 那档不该连累联网额度"
    );

    // ★ **提交不算查询**（2026-09-27 活实例实测出来的那条）：
    //   额度用光之后 `comfy_submit` 必须**照样能走** —— 否则"AI 自己搭一份工作流"这条
    //   正路会在最后一步被挡住（当时它侦察花掉 9 次，工作流写好了却提交不出去）。
    assert!(!crate::ai::ai_settings::is_comfy_query("comfy_submit"));
    assert_eq!(
        crate::ai::ai_settings::category_of("comfy_submit"),
        "comfy",
        "界面分组照旧归 comfy"
    );
    let before = session.comfy_queries_in_window;
    let calls_before = session.calls_in_window;
    session.spend_call("comfy_submit");
    assert_eq!(session.comfy_queries_in_window, before, "提交不占这份额度");
    assert_eq!(
        session.calls_in_window,
        calls_before + 1,
        "但总调用额度照旧要记"
    );
    // 同理：`comfy_use_media` / `comfy_make_mask` 是**写**（投放像素/遮罩），也不算"查询"？
    // —— 不，它们**算**（与 Kotlin 的 ToolCategory.COMFY 一致，而且它们会真的碰实例）。
    assert!(crate::ai::ai_settings::is_comfy_query("comfy_use_media"));
    assert!(crate::ai::ai_settings::is_comfy_query("comfy_make_mask"));

    // 完成作答 → 与联网同一档归零
    session.reset_budget();
    assert!(session.has_comfy_queries_left());
    assert_eq!(session.comfy_queries_in_window, 0);
}

/// 自动提交工作流次数按 Run 累计；答题重置只清"窗内查询额度"，
/// 不放宽这条硬上限（否则问一个问题就能继续无人值守地跑）。
#[test]
fn 自动提交额度按次累计且答题重置不放宽() {
    let mut session = ToolSession::default();
    assert!(session.has_auto_submits_left(0), "0 = 不限制，永远有额度");
    assert!(session.has_auto_submits_left(2));
    session.spend_auto_submit();
    assert!(session.has_auto_submits_left(2), "用了 1 次还剩 1 次");
    session.spend_auto_submit();
    assert!(
        !session.has_auto_submits_left(2),
        "用满 2 次之后必须退回询问"
    );
    session.reset_budget();
    assert!(
        !session.has_auto_submits_left(2),
        "答题重置不许把自动提交次数也清零"
    );
}

#[test]
fn missing_arguments_are_reported_not_unwrapped() {
    let err = string_arg(&json!({}), "name").unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(err.message.contains("name"));
    // 类型不对也算缺（模型给 null / 数字都要挡住）
    assert!(string_arg(&json!({"name": 7}), "name").is_err());
    assert!(string_arg(&json!({"name": null}), "name").is_err());
    assert_eq!(string_arg(&json!({"name": "x"}), "name").unwrap(), "x");
}

#[test]
fn mutating_flags_come_from_the_generated_catalog() {
    assert!(is_mutating("remember"));
    assert!(!is_mutating("list_skills"));
    assert_eq!(default_access("civitai_search"), "ASK");
    assert_eq!(default_access("list_skills"), "ALLOW");
    // 不认识的工具：按最保守的来（ASK）
    assert_eq!(default_access("nope"), "ASK");
    assert!(is_implemented("web_search"));
    assert!(is_implemented("civitai_search"));
    assert!(is_implemented("remember"));
}

/// 桌面文件工具五件（P4 第 40 步）：出厂档 / 写标记必须与 Kotlin 逐条一致
/// （档位错了的后果是"删 skill 不问一声"或者"写文件被静默禁掉"）。
#[test]
fn file_tool_defaults_match_the_kotlin_registry() {
    for name in ["list_dir", "read_file", "write_file", "register_skill"] {
        assert!(is_implemented(name), "{name} 应当已实现");
        assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
    }
    assert_eq!(default_access("delete_skill"), "ASK", "删 skill 要用户点头");
    assert!(is_mutating("write_file"));
    assert!(is_mutating("register_skill"));
    assert!(is_mutating("delete_skill"));
    assert!(!is_mutating("read_file"));
    assert!(!is_mutating("list_dir"));
    // comfy 查询 / 同步四件（P4 第 41~43 步）
    for name in [
        "comfy_get_run",
        "comfy_get_status",
        "comfy_list_workflows",
        "comfy_load_workflow",
    ] {
        assert!(is_implemented(name), "{name} 应当已实现");
        assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
        assert!(!is_mutating(name), "{name} 是只读的");
    }
    // `comfy_sync_history` 会写库 → 出厂 ASK（确认后才执行）
    assert!(is_implemented("comfy_sync_history"));
    assert_eq!(default_access("comfy_sync_history"), "ASK");
    assert!(is_mutating("comfy_sync_history"));
    // `comfy_submit`（P4 第 48 步，R10）：**会烧显卡时间** → 出厂 ASK，一个字都不能松
    assert!(is_implemented("comfy_submit"));
    assert_eq!(
        default_access("comfy_submit"),
        "ASK",
        "提交必须默认要用户批准"
    );
    assert!(is_mutating("comfy_submit"));
    // `comfy_env_report`（本机模型清单对账）：**只读**，出厂 ALLOW ——
    // 它不属于 COMFY 那一类（只扫盘 + 问一次 /object_info），不该吃掉查询预算
    assert!(is_implemented("comfy_env_report"));
    assert_eq!(default_access("comfy_env_report"), "ALLOW");
    assert!(!is_mutating("comfy_env_report"));
    // `comfy_diagnose_workflow`（"对于错误接线要能指出"）：
    // **只读**（读文件 / 读库 / 问一次 /object_info），出厂 ALLOW ——
    // 它是"检查一份图接线是否正确"，不需要每次都等待确认
    assert!(is_implemented("comfy_diagnose_workflow"));
    assert_eq!(default_access("comfy_diagnose_workflow"), "ALLOW");
    assert!(!is_mutating("comfy_diagnose_workflow"));
    // `comfy_find_workflow`（两段式找工作流）：**只读**，出厂 ALLOW ——
    // 它是系统提示第 16 条① 点名的那件（在此之前那句话是空的）
    assert!(is_implemented("comfy_find_workflow"));
    assert_eq!(default_access("comfy_find_workflow"), "ALLOW");
    assert!(!is_mutating("comfy_find_workflow"));
    // `comfy_list_templates`（官方模板，2026-11-07）：**只读本机 site-packages**，
    // 不联网、不写盘 → 出厂 ALLOW。新模型首次使用时 workflows 仍为空，
    // 模板是"这个模型官方怎么跑"的唯一可靠答案，不该每查一次都等批准。
    assert!(is_implemented("comfy_list_templates"));
    assert_eq!(default_access("comfy_list_templates"), "ALLOW");
    assert!(!is_mutating("comfy_list_templates"));
    // 体检两件（P4 第 62 步）：**只报不改** → 出厂 ALLOW、mutating=false
    // （Kotlin 那两条写的都是 `mutating = false, defaultAccess = ToolAccess.ALLOW`）
    for name in ["comfy_check_inputs", "comfy_fit_workflow"] {
        assert!(is_implemented(name), "{name} 应当已实现");
        assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
        assert!(!is_mutating(name), "{name} 只报不改");
    }
    // 审核 + 摘拼两件（P4 第 63 步）：同样**只报不改**（摘出来的图是**内联**返回的，
    // 实际落盘需由 write_file 或在 ComfyUI 中完成）→ 出厂 ALLOW、mutating=false
    for name in ["comfy_audit_workflow", "comfy_prune_workflow"] {
        assert!(is_implemented(name), "{name} 应当已实现");
        assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
        assert!(!is_mutating(name), "{name} 只报不改");
    }
    // 联网三件 + 定向查询（P4 第 64 步）：**出厂全部 ASK**（查询词会离开本机，
    // 确认后才发送），且全部只读（`download_model` 的写盘路由保持 deny）。
    // 档位改成 ALLOW 的后果是"查询词静默出本机"，界面上不会有任何变化。
    for name in ["web_search", "web_fetch", "civitai_search", "find_model"] {
        assert!(is_implemented(name), "{name} 应当已实现");
        assert_eq!(default_access(name), "ASK", "{name} 的出厂档必须是 ASK");
        assert!(!is_mutating(name), "{name} 是只读的（绝不代下）");
    }
    // 配方三件（P4 第 67 步）：学是**会写本机配方库**的（所以 mutating=true），
    // 但出厂档与 Kotlin 一致是 ALLOW（知识积累不该每一步都等批准）；
    // 列与绑只读（绑**只报不改**：它不提交、不写文件）。
    for name in [
        "comfy_learn_workflow",
        "comfy_list_recipes",
        "comfy_bind_recipe",
    ] {
        assert!(is_implemented(name), "{name} 已经搬过来了");
        assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
    }
    assert!(
        is_mutating("comfy_learn_workflow"),
        "学会往本机配方库写一行"
    );
    assert!(!is_mutating("comfy_list_recipes"));
    assert!(!is_mutating("comfy_bind_recipe"), "绑只报不改");
}

/// 设置页那份清单（`implemented`）与下发那份（`catalog`）的差别只有 `deny` 一条。
#[test]
fn deny_hides_a_tool_from_the_model_but_not_from_the_settings_page() {
    let mut config = crate::tools::policy::ToolPolicyConfig::default();
    config.overrides.insert("write_file".into(), "deny".into());
    let policy =
        crate::tools::policy::ToolPolicy::new(std::path::PathBuf::from(r"D:\proj"), config);

    let sent: Vec<&str> = catalog(&policy).iter().map(|t| t.name).collect();
    assert!(
        !sent.contains(&"write_file"),
        "被禁用的工具不该下发给模型：{sent:?}"
    );
    let listed: Vec<&str> = implemented().iter().map(|t| t.name).collect();
    assert!(
        listed.contains(&"write_file"),
        "设置页必须能看见被禁用的工具，否则再也开不回来：{listed:?}"
    );
    // 两份都只含已实现的
    assert_eq!(sent.len() + 1, listed.len());
    assert!(listed.contains(&"read_file") && sent.contains(&"read_file"));
}

#[test]
fn 参数被多包一层时在入口解开() {
    let inner = json!({
        "path": "D:\\proj\\comfyui\\krea2_r2i_run3.json",
        "mode": "overwrite",
        "content": "{\"1\":{\"class_type\":\"VAEDecode\"}}",
    });
    // 见过的最深一层：`{"arguments": "\"{\\\"path\\\":…}\""}`
    let twice = serde_json::to_string(&serde_json::to_string(&inner).unwrap()).unwrap();
    let raw = serde_json::to_string(&json!({ "arguments": twice })).unwrap();
    let fixed = normalize_arguments_text(&raw);
    let parsed: Value = serde_json::from_str(&fixed).unwrap();
    assert_eq!(parsed, inner, "解开之后 path / content 都要回来：{fixed}");

    // 正常参数一个字都不许动（比较值，不比较键序）
    let plain = r#"{"path":"a.md","reason":"先读一下"}"#;
    assert_eq!(
        serde_json::from_str::<Value>(&normalize_arguments_text(plain)).unwrap(),
        serde_json::from_str::<Value>(plain).unwrap()
    );
    // 解不出来就原样返回，别吞成 {}
    let broken = r#"{"arguments":"not json"}"#;
    assert_eq!(normalize_arguments_text(broken), broken);
}

/// 2026-11-12 的现场：模型把整包参数和 `reason` 写在**同一个对象**里。
///
/// 旧判据是"对象只有一个键才解"，这种两个键的对象会被整包放过；紧接着
/// `take_reason` 摘掉 `reason` 并重新序列化，工具层就只看到 `{"arguments": …}` ——
/// 于是 `comfy_submit` 明明收到了 `workflowPath`，却报「要么给 promptId…要么给
/// workflowPath」（同一次 Run 里连报 8 次）。
#[test]
fn 参数被多包一层时带着_reason_也要解开() {
    use crate::tools::tools::normalize_arguments_text;

    // ① 库里的真实形状：`{"arguments":"{\"promptId\":1705}","reason":"…"}`
    let raw = r#"{"arguments":"{\"promptId\":1705}","reason":"最小参数再试一次"}"#;
    let parsed: Value = serde_json::from_str(&normalize_arguments_text(raw)).unwrap();
    assert_eq!(parsed["promptId"], 1705, "promptId 必须回来：{parsed}");
    assert_eq!(
        parsed["reason"], "最小参数再试一次",
        "reason 要跟着解出来的对象一起回去（工具卡标题靠它）：{parsed}"
    );

    // ② 再套一层字符串的写法（`"\"{…}\""`）同样要解开
    let inner = json!({ "workflowPath": "D:\\Comfy-Desktop\\ComfyUI\\krea2 t2i workflow.json" });
    let raw = serde_json::to_string(&json!({
        "arguments": serde_json::to_string(&serde_json::to_string(&inner).unwrap()).unwrap(),
        "reason": "先跑一张试通",
    }))
    .unwrap();
    let parsed: Value = serde_json::from_str(&normalize_arguments_text(&raw)).unwrap();
    assert_eq!(parsed["workflowPath"], inner["workflowPath"]);
    assert_eq!(parsed["reason"], "先跑一张试通");

    // ③ 正常参数里出现 `arguments` 这个**键名**时不许解（工具可能真叫这个名字）
    let legit = r#"{"arguments":"x","path":"a.md"}"#;
    let parsed: Value = serde_json::from_str(&normalize_arguments_text(legit)).unwrap();
    assert_eq!(parsed["path"], "a.md");
    assert_eq!(parsed["arguments"], "x");
}

/// 节点契约查询（`nodes` 一族）走**独立档**：不占"别把 ComfyUI 当轮询器"的 9 次额度。
///
/// 现场（2026-10-25 第二轮）：查了两次节点类型，`comfy_submit` 之前就撞上
/// `QUERY_BUDGET_EXCEEDED` —— 而它问的是 `/object_info` 那份 schema，不是运行状态。
#[test]
fn 节点契约查询不占_comfy_查询额度() {
    use crate::ai::ai_settings::{category_of, is_comfy_query};
    assert_eq!(category_of("nodes"), "comfy", "界面分组照旧归 comfy");
    assert!(!is_comfy_query("nodes"), "schema 查询不该吃那 9 次");
    assert!(!is_comfy_query("nodes_upstream"), "同一族都豁免");
    // 真会碰实例状态的查询照旧要占
    assert!(is_comfy_query("comfy_get_status"));
    assert!(is_comfy_query("get_logs"), "日志不是 schema，仍算查询");

    let mut session = ToolSession::default();
    session.spend_call("nodes");
    session.spend_call("nodes");
    assert_eq!(session.comfy_queries_in_window, 0);
    assert_eq!(session.calls_in_window, 2, "总调用额度照记");
}

/// 按设定图逐部位确认（2026-11-04）：`ask_user_question` 必须能把设定图裁成
/// **一条真附件**（题目配图），并让选项图与选项文字一一对齐。
///
/// 为什么这条要在 `question_specs` 这一层钉：裁图发生在**开卡之前**，
/// 出错时不应占用一次额度；且选项图错位会导致"齐刘海"被看成"斜刘海"。
#[test]
fn 带图提问把设定图裁成附件_选项图与文字对齐() {
    use crate::media::attachments;
    use crate::web::routes::AppState;

    let dir = crate::infra::testpaths::scratch(&format!("ask-images-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let conn = crate::infra::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql"),
    )
    .unwrap();
    conn.execute_batch(&schema).unwrap();
    let state = AppState::new(conn, ":memory:", dir.join("media"));

    // 20×10 的真 PNG（裁左半 = 10×10）
    let mut image = image::RgbImage::new(20, 10);
    for pixel in image.pixels_mut() {
        *pixel = image::Rgb([10, 20, 200]);
    }
    let mut buffer = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut buffer, image::ImageFormat::Png)
        .unwrap();
    let tmp = attachments::temp_file(&state).unwrap();
    std::fs::write(&tmp, buffer.into_inner()).unwrap();
    let sheet = attachments::save(&state, "设定图.png", &tmp, Some("image/png")).unwrap();

    let args = json!({
        "questions": [{
            "question": "刘海是哪一种？",
            "header": "发型",
            "image": { "attachmentId": sheet.id, "region": "0,0,0.5,1" },
            "options": [
                { "text": "平直覆额的齐刘海", "imageUrl": "https://example.com/bangs.png" },
                "偏分的长斜刘海"
            ]
        }]
    });
    let specs = question_specs(&state, &args).expect("参数应当能解析");
    assert_eq!(specs.len(), 1);
    let spec = &specs[0];
    assert_eq!(spec.options, vec!["平直覆额的齐刘海", "偏分的长斜刘海"]);
    assert_eq!(spec.option_images.len(), 2, "示意图必须与选项一一对齐");
    assert_eq!(spec.option_images[0], "https://example.com/bangs.png");
    assert_eq!(spec.option_images[1], "", "没图的选项留空占位");

    let url = spec.image_url.as_deref().expect("题目要带裁图");
    let crop_id = url
        .strip_prefix("/api/ai/attachments/")
        .and_then(|rest| rest.strip_suffix("/file"))
        .expect("裁图地址必须是附件原址");
    let crop = attachments::get(&state, crop_id).unwrap().unwrap();
    assert_eq!(
        (crop.width, crop.height),
        (Some(10), Some(10)),
        "裁的必须是左半块"
    );
    assert_eq!(crop.name, "问题「刘海是哪一种？」.png");

    // 非 http(s) 的选项图直接报参数错（别把 file:// 之类塞给 <img>）
    let bad = json!({
        "questions": [{
            "question": "刘海？",
            "options": [{ "text": "齐刘海", "imageUrl": "file:///C:/x.png" }]
        }]
    });
    let error = question_specs(&state, &bad).unwrap_err();
    assert_eq!(error.code, "INVALID_ARGUMENT");
    assert!(error.message.contains("http(s)"), "{}", error.message);

    std::fs::remove_dir_all(dir).ok();
}

/// T1：`comfy_recent_successes` 只回 `status='success'` 且 `raw` 非空的行；
/// 能按 task / modelQuery 过滤，按时间倒序，limit 截断；`error` / 空 raw 绝不算"成功过"。
#[test]
fn comfy_recent_successes_只回成功且可按任务与模型族过滤() {
    use crate::web::routes::AppState;

    fn t2i(model: &str) -> serde_json::Value {
        json!({
            "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": model}},
            "2": {"class_type": "CLIPTextEncode", "inputs": {"text": "a cat", "clip": ["1", 1]}},
            "3": {"class_type": "KSampler", "inputs": {"seed": 1, "model": ["1", 0], "positive": ["2", 0], "negative": ["2", 0], "latent_image": ["4", 0]}},
            "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 512, "height": 512, "batch_size": 1}},
            "5": {"class_type": "VAEDecode", "inputs": {"samples": ["3", 0], "vae": ["1", 2]}},
            "6": {"class_type": "SaveImage", "inputs": {"filename_prefix": "t", "images": ["5", 0]}}
        })
    }
    fn i2i() -> serde_json::Value {
        json!({
            "1": {"class_type": "UNETLoader", "inputs": {"unet_name": "krea2TurboOfficialComfy_krea2TurboInt8.safetensors", "weight_dtype": "default"}},
            "2": {"class_type": "CLIPLoader", "inputs": {"clip_name": "qwen_3_06b_base.safetensors", "type": "krea2"}},
            "3": {"class_type": "LoadImage", "inputs": {"image": "input.png"}},
            "4": {"class_type": "VAEEncode", "inputs": {"pixels": ["3", 0], "vae": ["5", 0]}},
            "5": {"class_type": "VAELoader", "inputs": {"vae_name": "ae.safetensors"}},
            "6": {"class_type": "KSampler", "inputs": {"seed": 2, "model": ["1", 0], "positive": ["7", 0], "negative": ["8", 0], "latent_image": ["4", 0]}},
            "7": {"class_type": "CLIPTextEncode", "inputs": {"text": "x", "clip": ["2", 0]}},
            "8": {"class_type": "CLIPTextEncode", "inputs": {"text": "", "clip": ["2", 0]}},
            "9": {"class_type": "VAEDecode", "inputs": {"samples": ["6", 0], "vae": ["5", 0]}},
            "10": {"class_type": "SaveImage", "inputs": {"filename_prefix": "t", "images": ["9", 0]}}
        })
    }
    fn insert(
        conn: &rusqlite::Connection,
        key: &str,
        status: &str,
        raw: Option<&str>,
        at: &str,
        title: &str,
    ) {
        conn.execute(
            "INSERT INTO capture_runs (run_key, status, title, raw, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![key, status, title, raw, at],
        )
        .unwrap();
    }

    let dir = crate::infra::testpaths::scratch("recent-successes");
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let conn = crate::infra::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql"),
    )
    .unwrap();
    conn.execute_batch(&schema).unwrap();

    insert(
        &conn,
        "anima-new",
        "success",
        Some(&t2i("silvermoonmixAnimaEvolved_v2329B_int8.safetensors").to_string()),
        "2026-11-08T10:00:00.000Z",
        "anima 10点",
    );
    insert(
        &conn,
        "krea-i2i",
        "success",
        Some(&i2i().to_string()),
        "2026-11-08T11:00:00.000Z",
        "krea 11点",
    );
    insert(
        &conn,
        "anima-old",
        "success",
        Some(&t2i("anima-base-v1.0.safetensors").to_string()),
        "2026-11-08T09:00:00.000Z",
        "anima 9点",
    );
    insert(
        &conn,
        "bad-error",
        "error",
        Some(&t2i("anima-base-v1.0.safetensors").to_string()),
        "2026-11-08T12:00:00.000Z",
        "error 不算",
    );
    insert(
        &conn,
        "no-raw",
        "success",
        None,
        "2026-11-08T13:00:00.000Z",
        "空 raw 不算",
    );

    let state = AppState::new(conn, ":memory:", dir.join("media"));

    // 默认：最近的 3 条 success（error / 空 raw 不进），时间倒序
    let out = comfy_recent_successes(&state, &json!({})).unwrap();
    let items = out.payload["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["runKey"], "krea-i2i");
    assert_eq!(items[1]["runKey"], "anima-new");
    assert_eq!(items[2]["runKey"], "anima-old");
    assert_eq!(items[0]["task"], "i2i");
    assert_eq!(items[1]["task"], "t2i");
    // 模型文件名来自 raw 图（analyze 的 slots），排序后第一个是 ae
    assert_eq!(items[0]["models"][0], "ae.safetensors");
    // T6：带回**当前实例**地址（记录本身不区分实例，要人自己核）
    let url = out.payload["instanceUrl"].as_str().unwrap();
    assert!(url.starts_with("http"), "instanceUrl 是个地址：{url}");
    assert!(
        out.content.contains(url) && out.content.contains("不区分实例"),
        "正文要说清这些记录不区分实例：{}",
        out.content
    );

    // task 过滤
    let out = comfy_recent_successes(&state, &json!({"task": "t2i"})).unwrap();
    let items = out.payload["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["runKey"], "anima-new");
    assert_eq!(items[1]["runKey"], "anima-old");

    // modelQuery：同族匹配（大小写不敏感）
    let out = comfy_recent_successes(&state, &json!({"modelQuery": "KREA2"})).unwrap();
    let items = out.payload["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["runKey"], "krea-i2i");

    // limit 截断
    let out = comfy_recent_successes(&state, &json!({"task": "t2i", "limit": 1})).unwrap();
    assert_eq!(out.payload["items"].as_array().unwrap().len(), 1);
    assert_eq!(out.payload["items"][0]["runKey"], "anima-new");

    // 非法 task 如实报错
    let error = comfy_recent_successes(&state, &json!({"task": "nope"})).unwrap_err();
    assert_eq!(error.code, "INVALID_ARGUMENT");

    std::fs::remove_dir_all(dir).ok();
}

/// 2026-11-13：`comfy_recent_successes` 必须分两组 —— 「应用内提交的」与
/// 「在 ComfyUI 里自己跑的」。只按时间取最近 N 条时，应用刚跑完的那条永远把用户
/// 自己跑的那条挤掉（用户原话："只会查找由应用提交的最近一条"）。
///
/// 另钉一条兜底：老库里的应用提交行 `submitted_by` 还是默认值，但本进程提交过的那几跑
/// （`state.submissions`）也要归到应用那组。
#[test]
fn comfy_recent_successes_按提交方分两组() {
    use crate::web::routes::AppState;

    fn graph(model: &str) -> serde_json::Value {
        json!({
            "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": model}},
            "2": {"class_type": "CLIPTextEncode", "inputs": {"text": "a cat", "clip": ["1", 1]}},
            "3": {"class_type": "KSampler", "inputs": {"seed": 1, "model": ["1", 0], "positive": ["2", 0], "negative": ["2", 0], "latent_image": ["4", 0]}},
            "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 512, "height": 512, "batch_size": 1}},
            "5": {"class_type": "VAEDecode", "inputs": {"samples": ["3", 0], "vae": ["1", 2]}},
            "6": {"class_type": "SaveImage", "inputs": {"filename_prefix": "t", "images": ["5", 0]}}
        })
    }
    fn insert(conn: &rusqlite::Connection, key: &str, model: &str, at: &str, submitted_by: &str) {
        conn.execute(
            "INSERT INTO capture_runs (run_key, status, title, raw, created_at, submitted_by) \
             VALUES (?1, 'success', ?1, ?2, ?3, ?4)",
            rusqlite::params![key, graph(model).to_string(), at, submitted_by],
        )
        .unwrap();
    }

    let dir = crate::infra::testpaths::scratch("recent-successes-groups");
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let conn = crate::infra::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql"),
    )
    .unwrap();
    conn.execute_batch(&schema).unwrap();

    // 应用刚跑完（最新）、用户在 ComfyUI 里跑的（更早）
    insert(
        &conn,
        "app-new",
        "anima-base.safetensors",
        "2026-11-13T10:05:00.000Z",
        "ComfyHub",
    );
    insert(
        &conn,
        "comfy-user",
        "krea2.safetensors",
        "2026-11-13T10:00:00.000Z",
        "ComfyUI",
    );
    // 老行（列默认值）+ 本进程提交过：按内存里的提交清单归到应用组
    insert(
        &conn,
        "memory-app",
        "anima-base.safetensors",
        "2026-11-13T09:30:00.000Z",
        "ComfyUI",
    );

    let state = AppState::new(conn, ":memory:", dir.join("media"));
    state.submissions.record(json!({
        "promptId": "memory-app",
        "submittedAt": "2026-11-13T09:30:00.000Z",
    }));

    let out = comfy_recent_successes(&state, &json!({})).unwrap();
    let app = out.payload["appItems"].as_array().unwrap();
    let comfy = out.payload["comfyItems"].as_array().unwrap();
    let keys = |list: &[serde_json::Value]| -> Vec<String> {
        list.iter()
            .map(|item| item["runKey"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(keys(app), vec!["app-new", "memory-app"], "{}", out.content);
    assert_eq!(keys(comfy), vec!["comfy-user"], "{}", out.content);
    assert_eq!(app[0]["origin"], "app");
    assert_eq!(comfy[0]["origin"], "comfyui");
    assert!(
        out.content.contains("【应用内提交】") && out.content.contains("【ComfyUI 里运行的】"),
        "正文要分两组写清楚：{}",
        out.content
    );

    std::fs::remove_dir_all(dir).ok();
}

/// 2026-11-13：结尾提问（`ask_follow_up`）是**非阻塞的变种** ——
/// ① 工具结果要求结束这一轮（`ends_turn`）；② 提问落库为 `blocking=0`；
/// ③ Run 收尾 / 后端重启都**不许**把它清成 expired（用户下次回来还能答）；
/// ④ 作答会回写进消息里的卡（重开会话不能又显示"待回答"）。
#[test]
fn ask_follow_up_是收尾提问_不阻塞也不随_run_失效() {
    use crate::web::routes::AppState;

    let dir = crate::infra::testpaths::scratch("ask-follow-up");
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let conn = crate::infra::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql"),
    )
    .unwrap();
    conn.execute_batch(&schema).unwrap();
    let state = AppState::new(conn, ":memory:", dir.join("media"));
    state
        .write(|tx| {
            tx.execute("INSERT INTO ai_conversations (id) VALUES ('c1')", [])?;
            tx.execute(
                "INSERT INTO ai_runs (id, conversation_id, status) VALUES ('r1','c1','running')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let out = ask_follow_up(
        &state,
        "r1",
        "c1",
        &json!({
            "questions": [
                {"question": "跑哪一份？", "header": "工作流", "options": ["应用内提交的那份", "ComfyUI 里跑的那份"]},
                {"question": "要几张？", "options": ["1", "4"], "multiSelect": true},
            ],
            "submitText": "请按这个继续",
        }),
    )
    .unwrap();
    assert!(out.ends_turn, "结尾提问必须结束这一轮");
    assert_eq!(out.payload["pending"], true);
    assert_eq!(out.payload["submitText"], "请按这个继续");
    assert_eq!(out.payload["askedAll"].as_array().unwrap().len(), 2);

    let row: (i64, String) = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT blocking, status FROM ai_questions WHERE run_id = 'r1' \
                  ORDER BY card_index LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(row.0, 0, "结尾提问不是阻塞式");
    assert_eq!(row.1, "pending");

    // Run 收尾与后端重启只清阻塞式那批
    crate::ai::questions::finalize_run(&state, "r1", "expired").unwrap();
    crate::ai::questions::expire_stale(&state).unwrap();
    let status: String = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT status FROM ai_questions WHERE run_id = 'r1' ORDER BY card_index LIMIT 1",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(status, "pending", "结尾提问要留到用户下次回来作答");
    let pending = state.with(crate::ai::questions::list_pending).unwrap();
    assert!(pending.is_empty(), "结尾提问不进「有人等着」总览");

    // 作答回写进消息里的卡：先造一条与真库同形的 tool_result 块
    let question_id: String = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT question_id FROM ai_questions WHERE run_id = 'r1' ORDER BY card_index LIMIT 1",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    let part_payload = json!({
        "name": "ask_follow_up",
        "ok": true,
        "askedAll": [{
            "questionId": question_id,
            "question": "跑哪一份？",
            "options": ["应用内提交的那份", "ComfyUI 里跑的那份"],
            "answered": false,
        }],
    });
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO ai_messages (id, conversation_id, role) VALUES ('m1','c1','assistant')",
                [],
            )?;
            tx.execute(
                "INSERT INTO ai_message_parts (message_id, ordinal, type, text, tool_call_id, json_payload) \
                 VALUES ('m1', 0, 'tool_result', '', 'call-1', ?1)",
                rusqlite::params![part_payload.to_string()],
            )?;
            Ok(())
        })
        .unwrap();
    let accepted = crate::ai::questions::record_answer(
        &state,
        &question_id,
        &crate::ai::questions::Answer {
            answer: "应用内提交的那份".into(),
            option_index: Some(0),
            option_indexes: Vec::new(),
        },
    )
    .unwrap();
    assert!(accepted);
    let payload_text: String = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT json_payload FROM ai_message_parts WHERE message_id = 'm1'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&payload_text).unwrap();
    assert_eq!(payload["askedAll"][0]["answered"], true);
    assert_eq!(payload["askedAll"][0]["answer"], "应用内提交的那份");

    std::fs::remove_dir_all(dir).ok();
}

/// 真库里的 `capture_runs.raw` 是**整条 `/history` 条目**（`{"prompt":[…], "outputs":…}`），
/// 不是裸节点图 —— 只认裸图的实现会在真库上"一条都扫不出来"（不报错）。
///
/// 2026-11-09 抽查真库抓到的：`raw` 顶层键是 `prompt` / `outputs` / `status` / `meta`。
#[test]
fn comfy_recent_successes_认得真库里的_history_形状() {
    use crate::web::routes::AppState;

    let graph = json!({
        "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "anima-base-v1.0.safetensors"}},
        "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "a cat", "clip": ["1", 1]}},
        "3": {"class_type": "KSampler", "inputs": {"seed": 1, "model": ["1", 0], "positive": ["6", 0], "latent_image": ["7", 0]}},
        "7": {"class_type": "EmptyLatentImage", "inputs": {"width": 512, "height": 512, "batch_size": 1}},
        "9": {"class_type": "SaveImage", "inputs": {"images": ["3", 0], "filename_prefix": "t"}}
    });
    let history_row = json!({
        "prompt": [7, "real-prompt-id", graph, {"client_id": "x"}, ["9"]],
        "outputs": { "9": { "images": [{ "filename": "unit.png" }] } },
        "status": { "status_str": "success", "completed": true }
    })
    .to_string();

    let dir = crate::infra::testpaths::scratch("recent-successes-history-shape");
    std::fs::create_dir_all(dir.join("media")).unwrap();
    let conn = crate::infra::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql"),
    )
    .unwrap();
    conn.execute_batch(&schema).unwrap();
    conn.execute(
        "INSERT INTO prompts (id, title, positive_prompt, source) VALUES (7, '真库形状', 'x', 'ComfyUI')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO capture_runs (run_key, status, title, raw, created_at, prompt_id) \
         VALUES ('history-shape', 'success', '真库形状', ?1, '2026-11-09T00:00:00.000Z', 7)",
        rusqlite::params![history_row],
    )
    .unwrap();

    let state = AppState::new(conn, ":memory:", dir.join("media"));
    let out = comfy_recent_successes(&state, &json!({"task": "t2i"})).unwrap();
    let items = out.payload["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "history 形状的 raw 要被认出来：{}",
        out.content
    );
    assert_eq!(items[0]["runKey"], "history-shape");
    assert_eq!(items[0]["promptId"], 7);
    assert_eq!(items[0]["models"][0], "anima-base-v1.0.safetensors");

    std::fs::remove_dir_all(dir).ok();
}
