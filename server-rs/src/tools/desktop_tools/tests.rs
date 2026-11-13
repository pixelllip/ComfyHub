//! `desktop_tools` 的单元测试（主体代码在 [`desktop_tools`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn 清单有序且全是桌面专属() {
    let mut names: Vec<&str> = DESKTOP_TOOLS.iter().map(|t| t.name).collect();
    let sorted = {
        let mut n = names.clone();
        n.sort();
        n
    };
    assert_eq!(names, sorted, "桌面工具必须按 name 升序");
    names.dedup();
    assert_eq!(names.len(), DESKTOP_TOOLS.len(), "工具名不许重复");
    for tool in DESKTOP_TOOLS {
        assert_eq!(
            tool.scope, "DESKTOP",
            "{} 的 scope 必须是 DESKTOP",
            tool.name
        );
        // R11：手机侧不许复用提交工具名 —— 桌面清单里当然也不能出现别家的名字
        assert!(!tool.name.starts_with("comfy_queue_submit"));
    }
}

#[test]
fn 每个_schema_都是能解析的对象() {
    for tool in DESKTOP_TOOLS {
        let value: serde_json::Value = serde_json::from_str(tool.input_schema)
            .unwrap_or_else(|e| panic!("{} 的 schema 不是 JSON: {e}", tool.name));
        assert_eq!(
            value["type"], "object",
            "{} 的 schema 顶层必须是 object",
            tool.name
        );
        assert!(!tool.description.is_empty(), "{} 的描述不许为空", tool.name);
    }
}

/// 这一批**刻意不实现**的功能，不要一并加回（`comfy_*` 那一批需要 ComfyUI 在场或转换栈）。
///
/// ⚠ 联网三件（`web_search` / `web_fetch` / `civitai_search`）**不在这里出现**是**对的**：
/// 它们是 `BOTH` 档，住在 `mobile_tools.rs`；这一条名单只描述**桌面专属**那份清单。
#[test]
fn 只列真搬过来了的那些() {
    let names: Vec<&str> = DESKTOP_TOOLS.iter().map(|t| t.name).collect();
    assert_eq!(
        names,
        vec![
            // 结尾提问（2026-11-13）：与 `ask_user_question` 不同，它不阻塞，问题抛在回复末尾
            "ask_follow_up",
            "comfy_audit_workflow",
            // 配方三件（P4 第 67 步）：学 → 列 → 绑。绑的那件按 name 升序排在体检之前
            "comfy_bind_recipe",
            "comfy_check_inputs",
            // 接线体检（2026-10-17）：看的是**接线本身**，界面格式也认 ——
            // 与 `comfy_audit_workflow`（只审 API 图、"跑起来会不会崩"）分工不同，两件都要在
            "comfy_diagnose_workflow",
            "comfy_env_report",
            "comfy_find_workflow",
            "comfy_fit_workflow",
            "comfy_get_run",
            "comfy_get_status",
            // 学（往本机配方库写一行）与列（只读）夹在 get_status 与 list_workflows 之间
            "comfy_learn_workflow",
            "comfy_list_recipes",
            // 官方模板（2026-11-07）：ComfyUI 自带的那一套；只读本机 site-packages，不联网
            "comfy_list_templates",
            "comfy_list_workflows",
            "comfy_load_workflow",
            "comfy_make_mask",
            "comfy_prune_workflow",
            // T1（2026-11-08）：按任务 / 模型族查"上次成功"，只读
            "comfy_recent_successes",
            // 起 / 停实例（「通过 CLI 启动 ComfyUI 总是失败」）：
            // 自己起就不用 comfy-cli 写项目外的那两个文件 —— 那条路必然 Errno 13。
            "comfy_start",
            "comfy_stop",
            "comfy_submit",
            "comfy_sync_history",
            "comfy_use_attachment",
            "comfy_use_media",
            // 看画廊产物（2026-10-29）：只读；把图交给执行器的 reference_attach 自动附给模型
            "comfy_view_outputs",
            "danbooru_lookup",
            "delete_skill",
            // Provider 诊断（2026-11-11）：只读，把事实交给当前可用的模型解释
            "diagnose_provider",
            // 定向查询（P4 第 64 步）：**绝不代下**，只给链接 + sha256 + 目标目录
            "find_model",
            "list_dir",
            "read_file",
            "read_skill_file",
            // 参考图搜索（2026-11-04）：按 Danbooru tag 找真实图片、下载成本机附件
            "reference_image_search",
            "register_skill",
            "save_character_card",
            "save_style_card",
            "search_character_cards",
            "search_prompts",
            "search_style_cards",
            "write_file",
        ]
    );
    // `BOTH` 档的联网三件不许混进桌面专属清单（混进来 `implemented()` 会把它们收两次）
    for elsewhere in ["web_search", "web_fetch", "civitai_search"] {
        assert!(
            !names.contains(&elsewhere),
            "{elsewhere} 是 BOTH 档，属于 mobile_tools.rs"
        );
    }
    // 配方三件（P4 第 67 步）已经搬过来了：**必须**在下发清单里
    // （第 65/66 步那两条 `absent` 断言到这里就该反过来 —— 留着它等于把已实现的说成没实现）
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
}

/// Provider 诊断是**只读**的：它只采集事实，解释与建议由可用模型给出，不写配置。
#[test]
fn 诊断工具只读且不需要审批() {
    let tool = DESKTOP_TOOLS
        .iter()
        .find(|t| t.name == "diagnose_provider")
        .expect("diagnose_provider 必须在桌面清单里");
    assert_eq!(tool.access, "ALLOW");
    assert!(!tool.mutating, "诊断不许写库 / 改 Provider");
    assert_eq!(tool.scope, "DESKTOP");
    assert!(
        !tool.description.contains("opencode")
            && !tool.description.contains("azure")
            && !tool.description.contains("openrouter"),
        "工具描述不许带厂商示例"
    );
}

/// 配方三件的出厂档（与 Kotlin `ToolRegistry.kt` 逐条一致）：
///
/// `comfy_learn_workflow` **会向本机配方库写入一行**（不是本地文件）→ 如实标注 `mutating`，
/// 但与 `comfy_load_workflow` 同档 `ALLOW`（知识积累不该每一步都等批准）；
/// 列与绑都**只读**（绑只产出"该改哪几个键"的报告，不提交、不写文件）。
/// 档位改为 `ASK` 的后果是"每学习一份工作流都需确认"，界面上看不出变慢的原因。
#[test]
fn 配方三件的出厂档与_kotlin_一致() {
    for name in [
        "comfy_learn_workflow",
        "comfy_list_recipes",
        "comfy_bind_recipe",
    ] {
        let tool = DESKTOP_TOOLS.iter().find(|t| t.name == name).expect(name);
        assert_eq!(tool.access, "ALLOW", "{name} 的出厂档");
    }
    assert!(
        DESKTOP_TOOLS
            .iter()
            .find(|t| t.name == "comfy_learn_workflow")
            .unwrap()
            .mutating,
        "学会写本机配方库，mutating 必须为 true（Kotlin 也是这么标的）"
    );
    for name in ["comfy_list_recipes", "comfy_bind_recipe"] {
        let tool = DESKTOP_TOOLS.iter().find(|t| t.name == name).expect(name);
        assert!(!tool.mutating, "{name} 只报不改，mutating 必须为 false");
    }
}

/// 起 / 停实例两件的出厂档：**均需批准**（启动占用显存的长跑进程、停止即终止进程，
/// 两件都不该由模型自己决定）。
///
/// 与 comfy-mcp 那条 `launch_comfyui` 的档位一致（Kotlin `McpToolPolicyTest` 也是 ASK）——
/// 已确定"权限跟随当前对话的权限设置"，因此这里只固定**出厂档**。
#[test]
fn 起停实例两件默认都要审批() {
    for name in ["comfy_start", "comfy_stop"] {
        let tool = DESKTOP_TOOLS.iter().find(|t| t.name == name).expect(name);
        assert_eq!(tool.access, "ASK", "{name} 的出厂档必须是 ASK");
        assert!(tool.mutating, "{name} 会起/杀进程，mutating 必须为 true");
    }
}

/// 图像编辑三件的出厂档：**均需批准**（前两件会向本机磁盘写入文件、
/// `comfy_make_mask` 会覆盖 input 目录里同名的遮罩）。
///
/// 这条检查"是否被一并改成 `ALLOW`"：写文件类工具静默变为无需批准，
/// 界面上不会有任何变化，只会在磁盘上发现多出几个文件。
#[test]
fn 图像编辑三件默认都要审批() {
    for name in ["comfy_use_attachment", "comfy_use_media", "comfy_make_mask"] {
        let tool = DESKTOP_TOOLS.iter().find(|t| t.name == name).expect(name);
        assert_eq!(tool.access, "ASK", "{name} 的出厂档必须是 ASK");
        assert!(tool.mutating, "{name} 会写文件，mutating 必须为 true");
    }
}
