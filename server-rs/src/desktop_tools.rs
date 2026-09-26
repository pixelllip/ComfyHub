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
        name: "comfy_load_workflow",
        description: r#"把**一份本机工作流文件**（ComfyUI 保存的 .json，或「导出（API）」得到的节点图）读进库里，换成一个能提交的 promptId。用户甩给你一个工作流文件路径时用它 —— 别再回答"我只能提交库里的 promptId"。
界面格式（nodes/links）会按 ComfyUI 的 /object_info 转成 API 节点图：普通节点、Reroute / Set-Get、旁路都会等价展开或改写。
剩下的纯前端节点默认**如实报错**；带 tolerateUnsupported=true 再来一次，我会把它们摘掉并给出**缺口清单**：`unresolvedInputs`（哪个输入还悬着）、`openInputs`（哪些连线型输入空着，缺的是什么类型）、`unsupportedNodes`（摘掉了哪些类）—— 照着清单用 comfy_submit(connections=…) 补线即可。
返回里有 promptId、runnable（能不能直接跑）与每个节点的 id / 可覆盖的输入名。
只读用户机器上的文件，不改它。"#,
        // ⚠ 这段 schema 是 `ToolRegistry.kt` 那一条的**逐字**拷贝（R12/R13）——
        //   `emulateFrontend` 的描述里写着"默认开着、会照前端行为等价改写"，而本后端
        //   **还没实现它**：所以结果是**在返回值里**如实补一句 warning（见 `workflow_load.rs`），
        //   而不是偷偷改描述。描述走 `r##"…"##` 是因为正文里有 `##`
        //   （`r#"…"#` 会在 `"##` 处提前收尾，编译期报一串看不懂的 token 错）。
        input_schema: r###"{"type":"object","properties":{"path":{"type":"string","description":"工作流文件的完整路径（绝对路径最稳）"},"title":{"type":"string","description":"库里显示的名字（可选，默认取文件名）"},"includeGraph":{"type":"boolean","description":"是否连完整 API 节点图一起返回（默认 false，只有要手写参数路径时才需要）"},"tolerateUnsupported":{"type":"boolean","description":"转换不了的前端节点不报错，改成摘掉并给出缺口清单（默认 false）"},"emulateFrontend":{"type":"boolean","description":"## 模拟用户行为 ##：默认 true = 照 ComfyUI 前端排队前的行为等价改写纯前端节点（广播填空 / 丢显示节点 / 补 SaveImage），改了什么都如实列出来；false = 一个字都不改、只如实报缺口"}},"required":["path"],"additionalProperties":false}"###,
        scope: "DESKTOP",
        access: "ALLOW",
        mutating: false,
    },
    DesktopToolSpec {
        name: "comfy_make_mask",
        description: r#"生成一张**局部重绘用的遮罩**放进 ComfyUI 的 input 目录（**白 = 要重新生成的地方**）。两种需求用同一个工作流，只是遮罩反过来：**保背景换人物** = 人物那块 region；**换背景保人物** = 同一个 region 再加 `invert=true`。给了 baseFilename（input 目录里的底图）就照它尺寸生成，**推荐**这么用。region 支持 full / left / right / top / bottom / center / person，或归一化的 "x,y,w,h"（例如 "0.3,0.05,0.4,0.9"）；`feather` 羽化边缘（把人物贴进新背景时 8~24 明显更自然）。**框是估的**：工具会回 approximate=true 与它实际用的框，不准就报新坐标重做 —— 不要指望它做分割，也不要让用户去 ComfyUI 里手画。"#,
        // ⚠ 逐字来自 Kotlin `ToolRegistry.kt` 的 `comfy_make_mask`（R12/R13）
        input_schema: r#"{"type":"object","properties":{"baseFilename":{"type":"string","description":"input 目录里的底图文件名（先用 comfy_use_media / comfy_use_attachment 投放）；给了就用它的尺寸"},"width":{"type":"integer","description":"没给 baseFilename 时的画布宽"},"height":{"type":"integer","description":"没给 baseFilename 时的画布高"},"region":{"type":"string","description":"重绘区：full（默认）/ left / right / top / bottom / center / person / \"x,y,w,h\"（归一化 0~1）"},"shape":{"type":"string","description":"rect（默认）或 ellipse"},"invert":{"type":"boolean","description":"反选：只保留这块、其它全重绘（换背景保人物用这个）"},"feather":{"type":"integer","description":"边缘羽化像素，默认 0；8~24 常用于把人物贴进新背景"},"filename":{"type":"string","description":"可选：固定的遮罩文件名（不给自己按参数算一个，重复调用得到同一个文件）"}},"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ASK",
        mutating: true,
    },
    DesktopToolSpec {
        name: "comfy_submit",
        description: r#"**把一个工作流提交给 ComfyUI 真的跑一次**（会消耗显卡时间，默认需要用户批准）。用 promptId 指定库里的工作流（先 comfy_find_workflow 或 comfy_load_workflow），或者直接用 workflowPath 指一份本机工作流文件（等价于先 comfy_load_workflow 再提交）。用 overrides 覆盖参数（键写成 节点id.输入名，例如 "3.steps"、"6.text"）。用 connections 补连线（键一样，值写成 ["上游节点id", 槽位]，例如 {"60.vae":["1",0]}）——comfy_load_workflow 报 runnable=false 时，照它给的 openInputs / unresolvedInputs 补。跑完后产物会自动入库到画廊。不要凭想象编造工作流；说不清要跑什么就先问用户。
**提交前会先做一次模型预检**（用户 2026-09-22 拍板）：要的 checkpoint / LoRA / VAE 当前这个实例加载不了时会**当场拒**并告诉你该怎么办 —— 可能出现在硬盘上但实例看不到（重启 ComfyUI 就好，**别去下载**，见 `comfy_env_report`），也可能本机确实没有（去 `civitai_search` 查 + 问用户）。被拒时**不要改个名字重试**：预检看的是最终那张图（overrides / connections 应用之后），换个名字只会在下一次再被拒。
**成人内容**：这一跑出的是 NSFW / R18 内容时必须显式写 `nsfw=true` ——产物会带上 NSFW 标记，用户在设置里开了「预览图模糊」时画廊与回复里的缩略图就会打码。正常内容**不要**打开它（那会让正常的图也被糊住）；也**不要**因为用户没提就默认关闭——你比用户更清楚自己这次在生成什么。"#,
        // ⚠ 逐字来自 Kotlin `ToolRegistry.kt` 的 `comfy_submit`（R12/R13）。
        //   正文里的 `**` 与换行都是描述的一部分 —— 模型读的就是这份文本。
        input_schema: r#"{"type":"object","properties":{"promptId":{"type":"integer","description":"库里提示词的 id（来自 comfy_find_workflow / comfy_load_workflow）"},"workflowPath":{"type":"string","description":"或者：本机工作流文件的完整路径（.json）"},"overrides":{"type":"object","description":"要覆盖的参数：{\"6.text\":\"新提示词\",\"3.steps\":30}","additionalProperties":true},"connections":{"type":"object","description":"要补的连线：{\"60.vae\":[\"1\",0]}（key 是 节点id.输入名，值是这个输入该接到哪个节点的哪个输出槽位）","additionalProperties":true},"title":{"type":"string","description":"这次运行的标题（给用户认，可选）"},"nsfw":{"type":"boolean","description":"可选：这一跑出的是成人内容（NSFW/R18）时填 true，产物会带标记、预览图会按用户设置打码；正常内容不要填"},"wait":{"type":"boolean","description":"是否等它跑完（默认 true；false 表示排上队就返回）"},"waitSeconds":{"type":"integer","description":"最长等多少秒，默认 240，最多 900"}},"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ASK",
        mutating: true,
    },
    DesktopToolSpec {
        name: "comfy_sync_history",
        description: r#"立刻同步一次 ComfyUI /history（**会往本项目库里写数据**，属于轻度写操作）。需要用户批准；用之前先说明你想同步什么。"#,
        input_schema: r#"{"type":"object","properties":{"reason":{"type":"string","description":"为什么要同步（给用户看的）"}},"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ASK",
        mutating: true,
    },
    DesktopToolSpec {
        name: "comfy_use_attachment",
        description: r#"把**用户随消息发来的图片附件**放进 ComfyUI 的 input 目录，返回它在那边的真实文件名。做图生图 / 参考图 / 结构参考 / 换脸时必须先用它：工作流里 LoadImage 的 image 输入**只能填 ComfyUI input 目录里真实存在的文件名**，你不能改也编不出一个。拿到 filename 之后再用 comfy_submit 覆盖 LoadImage 节点的输入，例如 overrides={"89.image":"<返回的 filename>"}。只接受图片附件；附件 id 见用户消息末尾的附件说明。"#,
        // ⚠ 逐字来自 Kotlin `ToolRegistry.kt` 的 `comfy_use_attachment`（R12/R13）
        input_schema: r#"{"type":"object","properties":{"attachmentId":{"type":"string","description":"用户消息里那张图片的附件 id（形如 att_xxx / 32 位十六进制）"},"filename":{"type":"string","description":"可选：希望它在 ComfyUI 里叫什么（不填就用原文件名；会自动加一段 id 后缀防重名）"}},"required":["attachmentId"],"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ASK",
        mutating: true,
    },
    DesktopToolSpec {
        name: "comfy_use_media",
        description: r#"把**画廊里的一个图片产物**放进 ComfyUI 的 input 目录，返回它在那边真实可用的文件名与尺寸。两阶段图像编辑的地基：阶段一生成的人物图、要保留的背景图都在画廊里，而 LoadImage 只能读 input 目录里的文件。拿到 filename 与 width/height 之后：底图用 comfy_submit 覆盖 `节点id.image`；要遮罩就用 `comfy_make_mask(baseFilename=…, region=…)`。（用户随消息**发来**的图走 `comfy_use_attachment`，别混。）"#,
        // ⚠ 逐字来自 Kotlin `ToolRegistry.kt` 的 `comfy_use_media`（R12/R13）
        input_schema: r#"{"type":"object","properties":{"mediaId":{"type":"integer","description":"画廊里的图片产物 id（用 media 列表 / 刚生成的 mediaIds 里拿）"},"filename":{"type":"string","description":"可选：希望它在 ComfyUI 里叫什么（不填就用原名；会自动加一段 id 后缀防重名）"}},"required":["mediaId"],"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ASK",
        mutating: true,
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
        name: "save_character_card",
        description: r#"把一个角色的**规范 tag 栈 / LoRA / 权重 / 别名 / 来源**存成角色卡。同名 = 同一张卡（**合并保存**：只有你这次给了值的字段会被覆盖，`tags` 是**追加去重**，不会抹掉已有的）。写卡前先 `danbooru_lookup` 核对：**只有本轮真的查过的 tag 才会被记上核对时间**，没查过就如实留空（别自己编一个时间）。删除或清空字段请让用户去界面上做。"#,
        // ⚠ 逐字来自 Kotlin `ToolRegistry.kt` 的 `save_character_card`（R12/R13）。
        //   注意 `\"用户口述\"` 这种转义在**裸字符串**里也是字面量 —— 它本来就是 JSON 正文的一部分。
        input_schema: r#"{"type":"object","properties":{"name":{"type":"string","description":"角色名（用户叫的名字；同名视为同一张卡）"},"nameEn":{"type":"string","description":"英文 / 罗马字写法"},"aliases":{"type":"string","description":"别名，逗号分隔（danbooru 的 other names 可以照抄）"},"characterTag":{"type":"string","description":"danbooru 角色 tag，形如 shinku_(nte)"},"seriesTag":{"type":"string","description":"作品 tag，形如 neverness_to_everness"},"tags":{"type":"string","description":"规范 tag 栈，逗号或换行分隔（外观 / 服装 / 动作基线；danbooru 的同现 tag 就是它）"},"negative":{"type":"string","description":"这个角色常踩的负面词"},"loraName":{"type":"string","description":"角色 LoRA 文件名（ComfyUI models/loras 下的那个名字）"},"loraWeight":{"type":"number","description":"建议权重（没把握就用工作流里的值）"},"loraTrigger":{"type":"string","description":"LoRA 触发词（可能多个，逗号分隔）"},"checkpoint":{"type":"string","description":"建议底模"},"referenceMediaId":{"type":"integer","description":"参考图（画廊里的产物 id）"},"source":{"type":"string","description":"来源：链接 / \"用户口述\" / \"danbooru\" —— 如实写，不许编"},"notes":{"type":"string","description":"备注（这个角色怎么画才对）"}},"required":["name"],"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ALLOW",
        mutating: true,
    },
    DesktopToolSpec {
        name: "search_character_cards",
        description: r#"查本机存下来的**角色卡**（角色的规范 tag 栈 + 配哪条 LoRA / 权重 + 别名 + 来源 + 最后一次跟 danbooru 核对的时间）。**写某个角色之前先查这里**：有卡就照卡写，不用重新查一遍。查不到再去 `danbooru_lookup` 核对 tag / 找同现 tag，然后 `save_character_card` 存下来。"#,
        input_schema: r#"{"type":"object","properties":{"query":{"type":"string","description":"关键字：角色名 / 别名 / 角色 tag / LoRA 名；不传就是列全部"},"limit":{"type":"integer","description":"最多几条，默认 5，上限 20"}},"additionalProperties":false}"#,
        scope: "DESKTOP",
        access: "ALLOW",
        mutating: false,
    },    DesktopToolSpec {
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
                "comfy_load_workflow",
                "comfy_make_mask",
                "comfy_submit",
                "comfy_sync_history",
                "comfy_use_attachment",
                "comfy_use_media",
                "delete_skill",
                "list_dir",
                "read_file",
                "register_skill",
                "save_character_card",
                "search_character_cards",
                "write_file",
            ]
        );
        for absent in [
            "comfy_find_workflow",
            "comfy_check_inputs",
            "comfy_fit_workflow",
            "comfy_env_report",
            "comfy_audit_workflow",
            "comfy_prune_workflow",
            "search_prompts",
            "danbooru_lookup",
            "comfy_learn_workflow",
            "find_model",
        ] {
            assert!(!names.contains(&absent), "{absent} 还没实现，不许提前列进来");
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
}
