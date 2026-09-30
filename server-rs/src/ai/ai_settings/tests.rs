//! `ai_settings` 的单元测试（主体代码在 [`ai_settings`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn 类别与权限档的线上字与_kotlin_一致() {
    // 界面按这些字选图标 / 画下拉框，拼错就是"一个都不认识"
    assert_eq!(category_of("ask_user_question"), "ask");
    assert_eq!(category_of("list_skills"), "skill");
    assert_eq!(category_of("load_skill"), "skill");
    assert_eq!(category_of("remember"), "memory");
    assert_eq!(category_of("civitai_search"), "web");
    // ⚠ `danbooru_lookup` 与 `find_model` 都是 **WEB 档**（Kotlin `ToolRegistry` 那两条
    // 写的就是）—— 改一位，"联网预算"与界面分组会同时走偏，而两者都不会有用例变红：
    // 预算是执行器按**类别**判的（`MAX_WEB_QUERIES_PER_RUN`），分组是界面按它画的。
    assert_eq!(category_of("danbooru_lookup"), "web");
    assert_eq!(category_of("find_model"), "web");
    assert_eq!(category_of("web_search"), "web");
    assert_eq!(category_of("web_fetch"), "web");
    assert_eq!(category_of("search_prompts"), "library");
    assert_eq!(category_of("save_character_card"), "library");
    assert_eq!(category_of("comfy_submit"), "comfy");
    assert_eq!(access_wire(crate::tools::policy::ToolAccess::Allow), "allow");
    assert_eq!(access_wire(crate::tools::policy::ToolAccess::Ask), "ask");
    assert_eq!(access_wire(crate::tools::policy::ToolAccess::Deny), "deny");
}

#[test]
fn policy_dto_的键集合就是_kotlin_那十一个() {
    let policy = ToolPolicy::new(PathBuf::from(r"D:\proj"), ToolPolicyConfig::default());
    let json = policy_dto(&policy, &[]);
    let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
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
    // GET 回的是**展开后的绝对路径**（不是用户当初填的 "comfyui" 那几个字）
    assert_eq!(json["writeRoots"][0], r"D:\proj\comfyui");
    assert_eq!(json["defaultWriteRoot"], r"D:\proj\comfyui");
    assert_eq!(json["permissionMode"], "ask");
    assert!(json["overrides"].as_object().unwrap().is_empty());
}
