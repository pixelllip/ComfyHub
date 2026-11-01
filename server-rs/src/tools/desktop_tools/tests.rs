//! `desktop_tools` 的单元测试（主体代码在 [`desktop_tools`](super) —— 2026-09-30 按用户要求与主体分离）。
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
        assert_eq!(tool.scope, "DESKTOP", "{} 的 scope 必须是 DESKTOP", tool.name);
        // R11：手机侧不许复用提交工具名 —— 桌面清单里当然也不能出现别家的名字
        assert!(!tool.name.starts_with("comfy_queue_submit"));
    }
}

#[test]
fn 每个_schema_都是能解析的对象() {
    for tool in DESKTOP_TOOLS {
        let value: serde_json::Value = serde_json::from_str(tool.input_schema)
            .unwrap_or_else(|e| panic!("{} 的 schema 不是 JSON: {e}", tool.name));
        assert_eq!(value["type"], "object", "{} 的 schema 顶层必须是 object", tool.name);
        assert!(!tool.description.is_empty(), "{} 的描述不许为空", tool.name);
    }
}

/// 这一批**故意不做**的事，别顺手加回来（`comfy_*` 那一大票要 ComfyUI 在场或要转换栈）。
///
/// ⚠ 联网三件（`web_search` / `web_fetch` / `civitai_search`）**不在这里出现**是**对的**：
/// 它们是 `BOTH` 档，住在 `mobile_tools.rs`；这一条名单只描述**桌面专属**那份清单。
#[test]
fn 只列真搬过来了的那些() {
    let names: Vec<&str> = DESKTOP_TOOLS.iter().map(|t| t.name).collect();
    assert_eq!(
        names,
        vec![
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
            "comfy_list_workflows",
            "comfy_load_workflow",
            "comfy_make_mask",
            "comfy_prune_workflow",
            // 起 / 停实例（2026-09-28 用户 bug「通过 CLI 启动 comfyui 总是失败」）：
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
            // 定向查询（P4 第 64 步）：**绝不代下**，只给链接 + sha256 + 目标目录
            "find_model",
            "list_dir",
            "read_file",
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
        assert!(!names.contains(&elsewhere), "{elsewhere} 是 BOTH 档，属于 mobile_tools.rs");
    }
    // 配方三件（P4 第 67 步）已经搬过来了：**必须**在下发清单里
    // （第 65/66 步那两条 `absent` 断言到这里就该反过来 —— 留着它等于把已实现的说成没实现）
    for present in ["comfy_learn_workflow", "comfy_list_recipes", "comfy_bind_recipe"] {
        assert!(names.contains(&present), "{present} 已经搬过来了，不许漏：{names:?}");
    }
}

/// 配方三件的出厂档（与 Kotlin `ToolRegistry.kt` 逐条一致）：
///
/// `comfy_learn_workflow` **会往本机配方库写一行**（不是用户的文件）→ 如实标 `mutating`，
/// 但与 `comfy_load_workflow` 同档 `ALLOW`（知识积累不该每一步都等批准）；
/// 列与绑都**只读**（绑只产出"该改哪几个键"的报告，不提交、不写文件）。
/// 档位改成 `ASK` 的后果是"每学一份工作流都要用户点一下"，界面上看不出为什么变卡。
#[test]
fn 配方三件的出厂档与_kotlin_一致() {
    for name in ["comfy_learn_workflow", "comfy_list_recipes", "comfy_bind_recipe"] {
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

/// 起 / 停实例两件的出厂档：**都要用户批准**（起一个占显存的长跑进程、停一个是杀进程，
/// 两件都不该由模型自己决定）。
///
/// 与 comfy-mcp 那条 `launch_comfyui` 的档位一致（Kotlin `McpToolPolicyTest` 也是 ASK）——
/// 用户拍板过"权限跟随当前对话的权限设置"，所以这里只钉**出厂档**。
#[test]
fn 起停实例两件默认都要审批() {
    for name in ["comfy_start", "comfy_stop"] {
        let tool = DESKTOP_TOOLS.iter().find(|t| t.name == name).expect(name);
        assert_eq!(tool.access, "ASK", "{name} 的出厂档必须是 ASK");
        assert!(tool.mutating, "{name} 会起/杀进程，mutating 必须为 true");
    }
}

/// 图像编辑三件的出厂档：**都要用户批准**（前两件往用户机器上写文件、
/// `comfy_make_mask` 会覆盖 input 目录里同名的遮罩）。
///
/// 这条盯的是"哪天有人顺手把它们改成 ALLOW"：写文件类工具静默变成无需批准，
/// 界面上不会有任何变化，用户只会在磁盘上发现多了几个文件。
#[test]
fn 图像编辑三件默认都要审批() {
    for name in ["comfy_use_attachment", "comfy_use_media", "comfy_make_mask"] {
        let tool = DESKTOP_TOOLS.iter().find(|t| t.name == name).expect(name);
        assert_eq!(tool.access, "ASK", "{name} 的出厂档必须是 ASK");
        assert!(tool.mutating, "{name} 会写文件，mutating 必须为 true");
    }
}
