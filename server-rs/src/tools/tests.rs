//! `tools` 的单元测试（主体代码在 [`tools`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn specs_only_include_implemented_tools() {
    let policy = crate::policy::ToolPolicy::new(
        crate::testpaths::scratch("policy-root"),
        crate::policy::ToolPolicyConfig::default(),
    );
    let specs = specs(&policy);
    for spec in &specs {
        assert!(
            IMPLEMENTED.contains(&spec.name.as_str()),
            "{} 没实现却下发了 —— 模型会去调一个必然失败的调用",
            spec.name
        );
        assert!(!spec.description.is_empty());
        assert!(spec.parameters.is_object(), "{} 的 schema 必须是对象", spec.name);
    }
    assert!(specs.iter().any(|s| s.name == "list_skills"));
    assert!(specs.iter().any(|s| s.name == "ask_user_question"), "提问闸门已搬，应当下发");
    // 联网三件 + 定向查询（P4 第 64 步）：**应当**出现在清单里 ——
    // 系统提示第 17 条点名的那四个工具到这一步才真的下发给模型（在此之前那一节指向
    // 的是模型看不见的工具，等于一句空话）。
    for expected in ["web_search", "web_fetch", "civitai_search", "find_model"] {
        assert!(specs.iter().any(|s| s.name == expected), "{expected} 应当下发：{:?}", specs.iter().map(|s| &s.name).collect::<Vec<_>>());
    }
    // 配方三件（P4 第 67 步）已经实现 → 与桌面那两件一样，必须同时出现在提示词与请求里
    // （在此之前那两条 `absent` 断言是对的：没实现的一律不许下发）
    for expected in ["comfy_learn_workflow", "comfy_list_recipes", "comfy_bind_recipe"] {
        assert!(specs.iter().any(|s| s.name == expected), "{expected} 应当下发");
    }
}

/// 联网额度是一条**独立的线**（Kotlin `ToolContext.webInWindow` +
/// `ToolPolicyConfig.DEFAULT_MAX_WEB_QUERIES_PER_RUN`）：只认联网类工具、
/// 只在**真跑成功**之后计数、用户答完提问时归零。
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
    for _ in 2..crate::policy::MAX_WEB_QUERIES_PER_RUN {
        session.spend_call("danbooru_lookup");
    }
    assert_eq!(session.web_queries_in_window, crate::policy::MAX_WEB_QUERIES_PER_RUN);
    assert!(!session.has_web_queries_left());

    // 用户真答了提问 → 联网额度也归零（与总调用额度同一档重置）
    session.reset_budget();
    assert!(session.has_web_queries_left());
    assert_eq!(session.web_queries_in_window, 0);
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

    // comfy_* 三件查询都算（它们是"顺手再确认一下"的重灾区）
    session.spend_call("comfy_get_status");
    session.spend_call("comfy_list_workflows");
    session.spend_call("comfy_get_run");
    assert_eq!(session.comfy_queries_in_window, 3);
    assert_eq!(
        session.web_queries_in_window, 1,
        "danbooru_lookup 走的是**联网**那份额度，两份额度互不串味"
    );

    for _ in 3..crate::policy::MAX_COMFY_QUERIES_PER_RUN {
        session.spend_call("comfy_get_status");
    }
    assert_eq!(session.comfy_queries_in_window, crate::policy::MAX_COMFY_QUERIES_PER_RUN);
    assert!(!session.has_comfy_queries_left(), "到上限后预检必须拦得住");
    assert!(session.has_web_queries_left(), "拦住 ComfyUI 那档不该连累联网额度");

    // ★ **提交不算查询**（2026-09-27 活实例实测出来的那条）：
    //   额度用光之后 `comfy_submit` 必须**照样能走** —— 否则"AI 自己搭一份工作流"这条
    //   正路会在最后一步被挡住（当时它侦察花掉 9 次，工作流写好了却提交不出去）。
    assert!(!crate::ai_settings::is_comfy_query("comfy_submit"));
    assert_eq!(crate::ai_settings::category_of("comfy_submit"), "comfy", "界面分组照旧归 comfy");
    let before = session.comfy_queries_in_window;
    let calls_before = session.calls_in_window;
    session.spend_call("comfy_submit");
    assert_eq!(session.comfy_queries_in_window, before, "提交不占这份额度");
    assert_eq!(session.calls_in_window, calls_before + 1, "但总调用额度照旧要记");
    // 同理：`comfy_use_media` / `comfy_make_mask` 是**写**（投放像素/遮罩），也不算"查询"？
    // —— 不，它们**算**（与 Kotlin 的 ToolCategory.COMFY 一致，而且它们会真的碰实例）。
    assert!(crate::ai_settings::is_comfy_query("comfy_use_media"));
    assert!(crate::ai_settings::is_comfy_query("comfy_make_mask"));

    // 用户真答了提问 → 与联网同一档归零
    session.reset_budget();
    assert!(session.has_comfy_queries_left());
    assert_eq!(session.comfy_queries_in_window, 0);
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
    for name in ["comfy_get_run", "comfy_get_status", "comfy_list_workflows", "comfy_load_workflow"] {
        assert!(is_implemented(name), "{name} 应当已实现");
        assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
        assert!(!is_mutating(name), "{name} 是只读的");
    }
    // `comfy_sync_history` 会写库 → 出厂 ASK（用户点头才动）
    assert!(is_implemented("comfy_sync_history"));
    assert_eq!(default_access("comfy_sync_history"), "ASK");
    assert!(is_mutating("comfy_sync_history"));
    // `comfy_submit`（P4 第 48 步，R10）：**会烧显卡时间** → 出厂 ASK，一个字都不能松
    assert!(is_implemented("comfy_submit"));
    assert_eq!(default_access("comfy_submit"), "ASK", "提交必须默认要用户批准");
    assert!(is_mutating("comfy_submit"));
    // `comfy_env_report`（本机模型清单对账）：**只读**，出厂 ALLOW ——
    // 它不属于 COMFY 那一类（只扫盘 + 问一次 /object_info），不该吃掉查询预算
    assert!(is_implemented("comfy_env_report"));
    assert_eq!(default_access("comfy_env_report"), "ALLOW");
    assert!(!is_mutating("comfy_env_report"));
    // `comfy_find_workflow`（两段式找工作流）：**只读**，出厂 ALLOW ——
    // 它是系统提示第 16 条① 点名的那件（在此之前那句话是空的）
    assert!(is_implemented("comfy_find_workflow"));
    assert_eq!(default_access("comfy_find_workflow"), "ALLOW");
    assert!(!is_mutating("comfy_find_workflow"));
    // 体检两件（P4 第 62 步）：**只报不改** → 出厂 ALLOW、mutating=false
    // （Kotlin 那两条写的都是 `mutating = false, defaultAccess = ToolAccess.ALLOW`）
    for name in ["comfy_check_inputs", "comfy_fit_workflow"] {
        assert!(is_implemented(name), "{name} 应当已实现");
        assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
        assert!(!is_mutating(name), "{name} 只报不改");
    }
    // 审核 + 摘拼两件（P4 第 63 步）：同样**只报不改**（摘出来的图是**内联**返回的，
    // 真要落盘得由 write_file 或用户在 ComfyUI 里做）→ 出厂 ALLOW、mutating=false
    for name in ["comfy_audit_workflow", "comfy_prune_workflow"] {
        assert!(is_implemented(name), "{name} 应当已实现");
        assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
        assert!(!is_mutating(name), "{name} 只报不改");
    }
    // 联网三件 + 定向查询（P4 第 64 步）：**出厂全部 ASK**（查询词会离开本机，
    // 用户点一下才发），且全部只读（`download_model` 那条写盘路由保持 deny）。
    // 档位改成 ALLOW 的后果是"查询词静默出本机"，界面上不会有任何变化。
    for name in ["web_search", "web_fetch", "civitai_search", "find_model"] {
        assert!(is_implemented(name), "{name} 应当已实现");
        assert_eq!(default_access(name), "ASK", "{name} 的出厂档必须是 ASK");
        assert!(!is_mutating(name), "{name} 是只读的（绝不代下）");
    }
    // 配方三件（P4 第 67 步）：学是**会写本机配方库**的（所以 mutating=true），
    // 但出厂档与 Kotlin 一致是 ALLOW（知识积累不该每一步都等批准）；
    // 列与绑只读（绑**只报不改**：它不提交、不写文件）。
    for name in ["comfy_learn_workflow", "comfy_list_recipes", "comfy_bind_recipe"] {
        assert!(is_implemented(name), "{name} 已经搬过来了");
        assert_eq!(default_access(name), "ALLOW", "{name} 的出厂档");
    }
    assert!(is_mutating("comfy_learn_workflow"), "学会往本机配方库写一行");
    assert!(!is_mutating("comfy_list_recipes"));
    assert!(!is_mutating("comfy_bind_recipe"), "绑只报不改");
}

/// 设置页那份清单（`implemented`）与下发那份（`catalog`）的差别只有 `deny` 一条。
#[test]
fn deny_hides_a_tool_from_the_model_but_not_from_the_settings_page() {
    let mut config = crate::policy::ToolPolicyConfig::default();
    config.overrides.insert("write_file".into(), "deny".into());
    let policy = crate::policy::ToolPolicy::new(std::path::PathBuf::from(r"D:\proj"), config);

    let sent: Vec<&str> = catalog(&policy).iter().map(|t| t.name).collect();
    assert!(!sent.contains(&"write_file"), "被禁用的工具不该下发给模型：{sent:?}");
    let listed: Vec<&str> = implemented().iter().map(|t| t.name).collect();
    assert!(
        listed.contains(&"write_file"),
        "设置页必须能看见被禁用的工具，否则再也开不回来：{listed:?}"
    );
    // 两份都只含已实现的
    assert_eq!(sent.len() + 1, listed.len());
    assert!(listed.contains(&"read_file") && sent.contains(&"read_file"));
}
