//! `nsfw_prompt` 的单元测试（主体代码在 [`nsfw_prompt`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn no_sentinels_left() {
    for (name, text) in [("persona", persona_block()), ("nsfw", nsfw_block())] {
        assert!(!text.contains('«'), "{} 里还有没替换的插值哨兵", name);
        assert!(!text.contains("${"), "{} 里还有没处理的 Kotlin 插值", name);
    }
    // 阈值确实插进去了（Kotlin 侧是 ${...}，这里必须变成数字）
    let persona = persona_block();
    assert!(persona.contains(&format!(
        "最多主动查询 {} 次",
        crate::tools::policy::MAX_COMFY_QUERIES_PER_RUN
    )));
    assert!(persona.contains(&format!(
        "最多联网 {} 次",
        crate::tools::policy::MAX_WEB_QUERIES_PER_RUN
    )));
    assert!(persona.contains(&format!(
        "最多新增 {} 条",
        crate::ai::memory::MAX_ENTRIES_PER_RUN
    )));
}

#[test]
fn blocks_are_not_empty_and_trimmed() {
    let persona = persona_block();
    assert!(persona.len() > 5000, "常驻段正文太短，多半是提取截断了");
    // ⚠ `PERSONA_BLOCK` 在 Kotlin 里**就是**以一个空行开头、以一个换行结尾
    // （`trimIndent` 只去掉首尾各一个空行）—— 这是真实输出，别"顺手 trim 掉"：
    // 它进的是系统提示，差一个换行就是两端不一样。
    assert!(persona.starts_with('\n'));
    assert!(persona.ends_with('\n'));
    assert!(!nsfw_block().starts_with('\n'));
    assert!(nsfw_block().contains("[MODE: UNRESTRICTED SANDBOX — STABLE]"));
    assert!(DISABLED_NOTE.contains("NSFW"));
    assert!(TITLE_INSTRUCTION.starts_with("【本次附加任务"));
    // 起标题指令里有那对标记（手机 harness 靠它摘标题）
    assert!(TITLE_INSTRUCTION.contains("[标题]"));
}

/// 第 19 条（图像编辑）在 2026-09-27 改过一次，改的是**实测结论**，不是措辞：
/// 这里把四件"少了就会退回错误行为"的话钉住。
///
/// 为什么钉在系统提示这一侧：skill 是**按需加载**的（模型不 `load_skill` 就读不到），
/// 所以"贴人物图首选 AI 遮罩 / 缺模型要先问用户下 / krea2 别用来做编辑"这几条
/// **必须在常驻提示里也有一句**，否则模型在没加载 skill 的那一轮会直接选错路。
#[test]
fn 第19条钉住图像编辑的四条实测结论() {
    let persona = persona_block();
    // ① 贴人物图首选 AI 软遮罩（要有那条抠图链），且要先查模型在不在
    assert!(
        persona.contains("refine_foreground=true"),
        "第 19 条少了 AI 软遮罩那条链"
    );
    assert!(
        persona.contains("不再随包带"),
        "要写明仓库不再随包带这类工作流、用户得自备"
    );
    assert!(
        persona.contains("mattingModels.usable"),
        "少了「先查模型在不在」这一步"
    );
    // ② 缺模型要指引用户下载，且**不许**硬跑 / 默默降级（颜色抠图那一档 2026-09-27 已删除）
    assert!(
        persona.contains("如实让用户下模型"),
        "少了「缺模型就指引用户下」"
    );
    assert!(persona.contains("静默下载"), "要写明为什么不能硬跑");
    assert!(
        persona.contains("也绝不降级") && !persona.contains("matteSource=\"model\""),
        "颜色抠图档已删除：不能再教模型传 matteSource，也不能留「降级成颜色抠图」的说法",
    );
    // ③ krea2 **不适合做编辑**（这条以前没写，提示里还在推荐它做编辑）
    assert!(
        persona.contains("不适合做编辑"),
        "krea2 那条实测结论必须在常驻提示里"
    );
    // ④ 「先想能不能不抠」那两条路
    assert!(
        persona.contains("先想能不能不抠"),
        "少了「不抠图更稳」那一步"
    );
}

/// 第 17 条点名的四个联网工具**必须真的实现了**（P4 第 64 步把那个洞补上了）。
///
/// 为什么钉在系统提示这一侧：那一节是**常驻提示**，里面写着"要用 `web_search` /
/// `find_model`"，而模型能看见的只有 `IMPLEMENTED` 里下发的那几个 ——
/// 少一个，那句纪律就是**空话**（这一批之前它点名的四个工具一个都没下发，
/// 模型按提示去找工具，只会拿到 `TOOL_NOT_FOUND`）。
#[test]
fn 第17条点名的四个联网工具都真的实现了() {
    let persona = persona_block();
    for name in ["web_search", "web_fetch", "civitai_search", "find_model"] {
        assert!(persona.contains(name), "第 17 条少了 {name} 这个名字");
        assert!(
            crate::tools::tools::is_implemented(name),
            "{name} 在系统提示里点了名，却没进 IMPLEMENTED —— 模型按提示去找它只会失败"
        );
    }
}

/// 第 17 条那句"共用这一个额度"必须点名**所有**吃这份预算的工具。
///
/// 预算不是按工具名硬编码的，是按**类别**扣的（`executor::handle_call` 里
/// `category_of(name) == "web"`），而 `category_of` 的 web 档有五个：
/// `web_search` / `web_fetch` / `civitai_search` / `danbooru_lookup` / `find_model`。
/// 2026-09-27 之前这里只写了三个（漏 `find_model`；`danbooru_lookup` 也一直没写）——
/// 模型会以为它们是免费的，于是烧掉额度（用户看到的是后面每次搜索都被拦）。
#[test]
fn 第17条那句共用额度点名了所有吃这份预算的工具() {
    let persona = persona_block();
    // 那句话现在跨了两行（工具名一行、"共用这一个额度"一行），所以取的是**整段窗口**，
    // 不是单行 —— 单行匹配只会看见后半句。
    let start = persona
        .find("一次回复最多联网")
        .expect("第 17 条少了那段联网纪律");
    let after = start
        + persona[start..]
            .find("共用这一个额度")
            .expect("第 17 条少了「共用这一个额度」那句");
    let end = persona[after..]
        .find('\n')
        .map(|offset| after + offset)
        .unwrap_or(persona.len());
    let window = &persona[start..end];
    for name in [
        "web_search",
        "web_fetch",
        "civitai_search",
        "danbooru_lookup",
        "find_model",
    ] {
        assert!(
            window.contains(name),
            "「共用这一个额度」漏了 {name}（它按类别真的吃这份预算）：{window}"
        );
        assert_eq!(
            crate::ai::ai_settings::category_of(name),
            "web",
            "{name} 不在这份预算里了？那这句话要跟着改"
        );
    }
}

/// 第 20 条（工作流配方）—— 2026-09-27 补进常驻段。
///
/// 它原来是**不能写**的：配方三件在 Rust 侧还没下发，写上去就是描述不存在的工具（R13）。
/// 第 67 步交付了 `workflow_recipe.rs`，前提满足了，于是 P4 第 69 步照 Kotlin v31 的原文
/// **逐字**抄进 `v33`（`gen_prompts.py` 已停用，正文真源就是本文件）。
/// 这条用例同时钉两件事：五步那几条关键纪律在，且点名的三件工具**真的实现了**。
#[test]
fn 第20条钉住配方规则且点名的三件工具都真的实现了() {
    let persona = persona_block();
    assert!(
        persona.contains("20. **换（新）模型时先看"),
        "第 20 条没进常驻段"
    );
    for needle in [
        "comfy_list_recipes",
        "skeletonHash",
        "comfy_learn_workflow",
        "tolerateUnsupported=true",
        "complete=false",
        "comfy_bind_recipe",
        "onDiskOnly",
        "substitute",
        "绝不编文件名",
        "不是可提交对象",
        "file:<sha256>",
    ] {
        assert!(persona.contains(needle), "第 20 条少了 {needle}");
    }
    for name in [
        "comfy_learn_workflow",
        "comfy_list_recipes",
        "comfy_bind_recipe",
    ] {
        assert!(
            crate::tools::tools::is_implemented(name),
            "{name} 进了系统提示，却没进 IMPLEMENTED —— 模型按提示去找它只会失败"
        );
    }
}

/// 第 24 条（成品提示词一律进 Markdown 代码块）—— 2026-10-28 用户明确要求。
///
/// 钉两件事：① 围栏语言写 `text` —— 聊天渲染器只对"散文类"语言标记做硬折行回收
/// （`markdown.dart` 的 `_proseCodeLanguages`），换成 `json` / `markdown` 之类会按代码原样
/// 保留换行，用户复制走的就是一行一断的提示词；② "只在正文里提一句不算交付"必须写着，
/// 否则模型容易把它当成排版建议而不是硬约束。
#[test]
fn 第24条要求成品提示词放进_text_代码块() {
    let persona = persona_block();
    assert!(
        persona.contains("24. **成品提示词一律放进 Markdown 代码块**"),
        "第 24 条没进常驻段（这条是改提示词必须一起 bump 的那类改动）"
    );
    assert!(
        persona.contains("```text"),
        "围栏语言要写 text：渲染器据此把模型硬折行的提示词拼回一段"
    );
    for needle in [
        "正面提示词",
        "负面提示词",
        "Danbooru tag 栈",
        "一版一块",
        "都不算交付",
    ] {
        assert!(persona.contains(needle), "第 24 条少了 {needle}");
    }
}
