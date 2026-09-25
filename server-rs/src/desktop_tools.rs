//! 桌面专属工具清单（`ToolScope = DESKTOP`）—— 这一批是**文件工具五件**。
//!
//! # 为什么它必须与 `mobile_tools.rs` 分开
//!
//! `mobile_tools.rs` 是**生成**的（`python server-rs/tools/gen_mobile_tools.py`，
//! 数据源是 Kotlin `/api/ai/run-kit` 的 `tools[]`），而 run-kit **只下发手机能执行的**
//! （🔒 `docs/tool-scope-contract.md`：手机清单里不许出现 `comfy_*` / 文件工具）。
//! 所以桌面专属的这些在 run-kit 里**取不到**，只能单独列一份。
//!
//! # 与 Kotlin 的关系（R12：工具清单只有一个真源）
//!
//! 每条 `description` / `input_schema` 都是 `ToolRegistry.kt` 里那一条的**逐字**拷贝。
//! 判据不是"看着差不多"，而是同一份库副本上跑 Kotlin 的 `GET /api/ai/tools`
//! （name / description / category / mutating / access）与这里的清单逐条比 ——
//! 集成用例 `desktop_tools_match_the_kotlin_catalog` 钉住这件事。
//!
//! ⚠ **没实现的不许出现在这里**：`IMPLEMENTED`（`tools.rs`）与这份清单的交集才下发。
//! 把 `comfy_submit` 提前列进来而没实现，模型就会去调一个必然失败的调用。
//!
//! # 这一批的边界
//!
//! 只搬**不碰网络、不碰 ComfyUI** 的五件：`list_dir` / `read_file` / `write_file`
//! （路径准入见 [`crate::policy::ToolPolicy::resolve_read`]）与 `register_skill` /
//! `delete_skill`（磁盘真源在 [`crate::skills`]）。剩下的大块 —— `comfy_*` 十三件、
//! 官方 comfy-mcp 那 39 个（要 `McpStdioClient`）、`find_model` —— 各自另起批次。

/// 一个桌面专属工具（字段与 [`crate::mobile_tools::MobileToolSpec`] 同形）。
pub struct DesktopToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema 原文（原样进下发清单的 `parameters`）。
    pub input_schema: &'static str,
    /// 线上字（桌面专属一律 `DESKTOP`）。
    pub scope: &'static str,
    /// 出厂权限档（`ALLOW` / `ASK` / `DENY`）；用户在设置里可以逐工具覆盖。
    pub access: &'static str,
    /// 会不会写数据（prompt 里"（会写数据）/（只读）"那个标记）。
    pub mutating: bool,
}

/// ⚠ **按 name 升序**：与手机清单合并之后仍要保持有序（系统提示里那两行的顺序要稳定）。
pub const DESKTOP_TOOLS: &[DesktopToolSpec] = &[
    DesktopToolSpec {
        name: "comfy_get_run",
        description: r#"按 runKey 查一次已捕获运行的状态、错误与产物数。runKey 可以用三样中的任何一个：ComfyUI 的 prompt_id（UUID，comfy_submit 结果里的 runKey）、捕获记录里的数字 id（capturedPromptId），或 import:<sha256>。"#,
        input_schema: r#"{"type":"object","properties":{"runKey":{"type":"string","description":"ComfyUI prompt_id（UUID）/ 数字 prompt_id / import:<sha256>"}},"required":["runKey"],"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ALLOW",
        mutating: false,
    },
    DesktopToolSpec {
        name: "comfy_get_status",
        description: r#"查询 ComfyUI 的连通性、队列（运行中 / 等待中）与最近的捕获记录。地址由应用配置决定，不接受任意 URL。"#,
        input_schema: r#"{"type":"object","properties":{"includeRecent":{"type":"boolean","description":"默认 true"},"recentLimit":{"type":"integer","description":"默认 5，最多 20"}},"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ALLOW",
        mutating: false,
    },
    DesktopToolSpec {
        name: "comfy_list_workflows",
        description: r#"列出**本机 ComfyUI 里已经保存的工作流文件**（`user\<用户>\workflows\*.json`），带完整路径、格式（ui/api）与是否已经在本项目库里（inLibrary / promptId）。**首次使用、或者 comfy_find_workflow 什么都搜不到时，先看这里** —— 这些工作流不需要在 ComfyUI 里跑过一次才可用：拿到 path 直接 comfy_load_workflow(path=…) 读进库，再用返回的 promptId 提交。只读，不改用户的文件。"#,
        input_schema: r#"{"type":"object","properties":{"query":{"type":"string","description":"按**文件名**过滤（大小写不敏感的子串，例如 krea2）；不填就列最近修改的若干份"},"limit":{"type":"integer","description":"最多列几份，默认 20，最多 100"}},"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ALLOW",
        mutating: false,
    },
    DesktopToolSpec {
        name: "delete_skill",
        description: r#"删除一个**用户来源**的 Skill。内置 skill 不能删。这是不可逆操作，用户确认后再调用。"#,
        input_schema: r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ASK",
        mutating: true,
    },
    DesktopToolSpec {
        name: "list_dir",
        description: r#"列目录（默认 ComfyUI 目录）。只允许读允许的目录，越界会被拒绝。"#,
        input_schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对项目根或绝对路径；缺省 = ComfyUI 目录"},"depth":{"type":"integer","description":"1 或 2，默认 1"}},"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ALLOW",
        mutating: false,
    },
    DesktopToolSpec {
        name: "read_file",
        description: r#"读一个文本文件（默认限制在允许读取的目录内）。超过上限会**分段读**：返回里会给出 nextOffset，按它再调用一次就能接着读，不要因为一次没读完就放弃。"#,
        input_schema: r#"{"type":"object","properties":{"path":{"type":"string"},"maxBytes":{"type":"integer","description":"可选，本次最多读多少字节（默认 2097152）"},"offset":{"type":"integer","description":"可选，从第几个字节开始读（默认 0；接着上一次的 nextOffset 读下一段）"}},"required":["path"],"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ALLOW",
        mutating: false,
    },
    DesktopToolSpec {
        name: "register_skill",
        description: r#"把一个新的 Skill 注册到本机（写入项目自己的 skills 目录，立刻生效，不用重启应用）。用户说"记住这个流程 / 注册一个 skill"时用它。name 用小写 kebab-case，正文写清步骤与判据。"#,
        input_schema: r#"{"type":"object","properties":{"name":{"type":"string","description":"kebab-case，例如 anima-prompt-helper"},"description":{"type":"string","description":"一句话说明这个 skill 干什么（必填，≤600 字）"},"whenToUse":{"type":"string","description":"什么情况下该用（可选）"},"content":{"type":"string","description":"Markdown 正文：完整规则与步骤"}},"required":["name","description","content"],"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ALLOW",
        mutating: true,
    },
    DesktopToolSpec {
        name: "write_file",
        description: r#"写文件。**默认只允许写 ComfyUI 目录**（`comfyui/`）；写到别处会被权限策略直接拒绝，不要试图绕开。写前先想清楚路径，必要时先 list_dir 看一眼。"#,
        input_schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"目标路径（相对项目根或绝对路径）"},"content":{"type":"string","description":"完整内容"},"mode":{"type":"string","enum":["overwrite","append"],"description":"默认 overwrite"}},"required":["path","content"],"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ALLOW",
        mutating: true,
    },
];

#[cfg(test)]
mod tests {
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

    /// 这一批**故意不做**的事，别顺手加回来（`comfy_submit` / `comfy_*` 那一大票要 ComfyUI
    /// 在场或要转换栈，归后面的批次）。
    #[test]
    fn 只列真搬过来了的那些() {
        let names: Vec<&str> = DESKTOP_TOOLS.iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                "comfy_get_run",
                "comfy_get_status",
                "comfy_list_workflows",
                "delete_skill",
                "list_dir",
                "read_file",
                "register_skill",
                "write_file",
            ]
        );
        for absent in [
            "comfy_submit",
            "comfy_sync_history",
            "comfy_load_workflow",
            "comfy_find_workflow",
            "comfy_env_report",
            "find_model",
        ] {
            assert!(!names.contains(&absent), "{absent} 还没实现，不许提前列进来");
        }
    }
}
