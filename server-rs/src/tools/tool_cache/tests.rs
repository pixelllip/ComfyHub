//! `tool_cache` 的单元测试（主体代码在 [`tool_cache`](super) —— 与仓库里其它模块同一条规矩：
//! 测试与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项。

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde_json::json;

use super::*;
use crate::tools::policy::{ToolPolicy, ToolPolicyConfig};

/// 缓存是**进程内全局**的，而同一个测试二进制里的用例是并行跑的 ——
/// 淘汰用例会把别的用例刚存的条目挤掉。这个锁把本模块的用例串起来，
/// 顺带在每个用例开头清一次场。
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    invalidate_all();
    guard
}

/// 一个只允许读 `<scratch>/data` 的策略 + 那棵目录树。
///
/// `scratch` 每次调用都是**新目录**（纳秒后缀），所以各用例的键天然不撞；
/// 落盘位置在 `target\.scratch\` 里（AGENTS §1：不许用 `temp_dir()`）。
fn policy_with(label: &str, read_roots: &[&str]) -> (ToolPolicy, PathBuf) {
    let root = crate::infra::testpaths::scratch(label);
    let data = root.join("data");
    std::fs::create_dir_all(&data).expect("建 data 目录");
    let config = ToolPolicyConfig {
        read_roots: read_roots.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    (ToolPolicy::new(root, config), data)
}

#[test]
fn 参数键序不同也算同一组参数() {
    let _guard = serial();
    let (policy, _) = policy_with("cache-key-order", &["data"]);
    store(&policy, "comfy_get_status", &json!({ "a": 1, "b": 2 }), "队列空", &json!({}));
    let hit = lookup(&policy, "comfy_get_status", &json!({ "b": 2, "a": 1 }))
        .expect("模型两次拼 JSON 的键序经常不一样，键序不该影响命中");
    assert!(hit.content.contains("队列空"));
    // 少一个键、或值不同 → 另一组参数
    assert!(lookup(&policy, "comfy_get_status", &json!({ "a": 1 })).is_none());
    assert!(lookup(&policy, "comfy_get_status", &json!({ "a": 1, "b": 3 })).is_none());
}

/// 纪律 2：命中**必须如实说**，而且模型与用户要**同时**看得见
/// （正文给模型，摘要 `preview` 就是工具卡上那一行小字）。
#[test]
fn 命中时正文与摘要都如实标注() {
    let _guard = serial();
    let (policy, _) = policy_with("cache-note", &["data"]);
    assert!(store(
        &policy,
        "comfy_get_status",
        &json!({}),
        "队列里有 2 个任务",
        &json!({ "queued": 2 }),
    ));
    let (content, preview, payload) = lookup(&policy, "comfy_get_status", &json!({}))
        .expect("应当命中")
        .into_wire();
    assert!(content.starts_with("[缓存命中]"), "正文必须如实说是缓存：{content}");
    assert!(content.contains("没有重新执行"), "必须写明没有重新执行：{content}");
    assert!(content.contains("队列里有 2 个任务"), "原结果一个字都不能少：{content}");
    assert!(preview.starts_with("[缓存命中]"), "工具卡上那一行也要看得见：{preview}");
    assert_eq!(payload["cached"], json!(true));
    assert_eq!(payload["queued"], json!(2), "工具自己的负载一个键都不能丢");
    assert!(payload["cachedAgeSeconds"].is_number(), "给界面一个能显示「多久之前」的数");
}

/// 名单是白名单不是黑名单：会改世界的、闸门、以及**自己有更严去重**的那几条
/// 一律不许进来；纯读工具一个都不能漏（漏了就是"设了缓存却没有缓存"）。
#[test]
fn 会改世界的工具一律不进缓存() {
    let _guard = serial();
    for name in [
        "write_file",
        "register_skill",
        "delete_skill",
        "remember",
        "comfy_submit",
        "comfy_use_media",
        "comfy_use_attachment",
        "comfy_make_mask",
        "comfy_start",
        "comfy_stop",
        "comfy_sync_history",
        "comfy_learn_workflow",
        "save_character_card",
        // ⚠ 它的 `mutating` 是 `false`（不改用户机器上的文件），但它会往库里写提示词 ——
        //   `invalidates` 里那条显式例外盯的就是它
        "comfy_load_workflow",
    ] {
        assert!(freshness(name).is_none(), "{name} 不该进缓存（它会改世界）");
        assert!(invalidates(name), "{name} 跑成功之后必须清缓存");
    }
    // 闸门（缓存一个提问 = 吞掉用户的选择）与"自己更严的去重"（`load_skill` 一轮只放行一次）
    for name in ["ask_user_question", "load_skill"] {
        assert!(freshness(name).is_none(), "{name} 不该进缓存");
    }
    // 反面：纯读工具既要能进缓存，又**不该**清缓存
    for name in
        ["read_file", "list_dir", "comfy_get_status", "comfy_get_run", "web_search", "find_model"]
    {
        assert!(freshness(name).is_some(), "{name} 应当进缓存");
        assert!(!invalidates(name), "{name} 不该清缓存");
    }
}

/// 磁盘类工具按 `mtime + 长度` 校验：用户在自己的编辑器里存一次，
/// 下一次调用就必须重新读（TTL 只能把窗口缩小，`stat` 能把它关掉）。
#[test]
fn 磁盘类工具按文件变动失效() {
    let _guard = serial();
    let (policy, data) = policy_with("cache-file-stat", &["data"]);
    let file = data.join("a.txt");
    std::fs::write(&file, "第一版").expect("写测试文件");
    let args = json!({ "path": file.display().to_string() });
    store(&policy, "read_file", &args, "第一版", &json!({}));
    assert!(lookup(&policy, "read_file", &args).is_some(), "没动过就该命中");

    // ① 长度变了
    std::fs::write(&file, "第二版（更长）").expect("覆盖测试文件");
    assert!(lookup(&policy, "read_file", &args).is_none(), "文件被改了就必须重新读");

    // ② 长度不变、只有 mtime 变（**显式设 mtime**，不靠"写得够快就同秒"这种巧合）
    std::fs::write(&file, "第三版").expect("覆盖测试文件");
    store(&policy, "read_file", &args, "第三版", &json!({}));
    assert!(lookup(&policy, "read_file", &args).is_some());
    let handle = std::fs::File::options().write(true).open(&file).expect("打开测试文件");
    handle
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        .expect("设置 mtime");
    drop(handle);
    assert!(lookup(&policy, "read_file", &args).is_none(), "只动 mtime 也算被动过");
}

/// 读白名单收紧之后，缓存**不能**继续把那份内容续出去 ——
/// 路径要过 `ToolPolicy::resolve_read` 才谈得上命中（这条是安全边界，不是省不省事）。
#[test]
fn 读白名单收紧之后不再供那份内容() {
    let _guard = serial();
    let (policy, data) = policy_with("cache-policy", &["data"]);
    let file = data.join("a.txt");
    std::fs::write(&file, "内容").expect("写测试文件");
    let raw = file.display().to_string();
    let args = json!({ "path": raw });
    store(&policy, "read_file", &args, "内容", &json!({}));
    assert!(lookup(&policy, "read_file", &args).is_some());

    let narrowed = ToolPolicy::new(
        policy.project_root.clone(),
        ToolPolicyConfig { read_roots: vec!["other".into()], ..Default::default() },
    );
    assert!(narrowed.resolve_read(&raw).is_err(), "前提：收紧之后这个路径本身就过不了准入");
    assert!(lookup(&narrowed, "read_file", &args).is_none(), "过不了准入的路径一律不许命中缓存");
}

/// TTL 到期就真跑（用例把钟往前拨，不 `sleep`）。
#[test]
fn 过了_ttl_就不再命中() {
    let _guard = serial();
    let (policy, _) = policy_with("cache-ttl", &["data"]);
    store(&policy, "comfy_get_status", &json!({}), "队列空", &json!({}));
    let now = Instant::now();
    assert!(
        lookup_at(&policy, "comfy_get_status", &json!({}), now + Duration::from_secs(1)).is_some(),
        "1 秒之后还在 TTL 里"
    );
    assert!(
        lookup_at(&policy, "comfy_get_status", &json!({}), now + Duration::from_secs(11)).is_none(),
        "10 秒的 TTL 过去之后必须真跑"
    );
}

/// 有界（纪律 5）：超上限先淘汰最老的，且条数卡死。
#[test]
fn 缓存条数有上限() {
    let _guard = serial();
    let (policy, _) = policy_with("cache-bound", &["data"]);
    for index in 0..(MAX_ENTRIES + 20) {
        store(&policy, "comfy_get_status", &json!({ "i": index }), "x", &json!({}));
    }
    assert_eq!(entry_count(), MAX_ENTRIES, "条数必须卡在上限");
    assert!(lookup(&policy, "comfy_get_status", &json!({ "i": 0 })).is_none(), "最老的先被淘汰");
    assert!(
        lookup(&policy, "comfy_get_status", &json!({ "i": MAX_ENTRIES + 19 })).is_some(),
        "最新的还在"
    );
}

/// 超长正文不进缓存（它本来就会被截断，再存一份只是白占内存）。
#[test]
fn 超长结果不进缓存() {
    let _guard = serial();
    let (policy, _) = policy_with("cache-big", &["data"]);
    let big = "字".repeat(MAX_CONTENT_CHARS + 1);
    assert!(!store(&policy, "comfy_get_status", &json!({}), &big, &json!({})));
    assert!(lookup(&policy, "comfy_get_status", &json!({})).is_none());
    assert_eq!(entry_count(), 0);
}

/// 负载不是对象时不许吞内容（包的形状与 `truncate_result` 那条同一手法）。
#[test]
fn 非对象负载也带上缓存标记() {
    let _guard = serial();
    let (policy, _) = policy_with("cache-payload", &["data"]);
    let args = json!({ "url": "https://example.com" });
    store(&policy, "web_fetch", &args, "正文", &json!("纯文本"));
    let (_, _, payload) = lookup(&policy, "web_fetch", &args).expect("命中").into_wire();
    assert_eq!(payload["cached"], json!(true));
    assert_eq!(payload["result"], json!("纯文本"), "非对象负载要原样留在 result 里");
}

/// 写操作之后缓存的答案就过期了 —— 清空是**唯一**的失效动作（纪律 4）。
#[test]
fn 写操作之后缓存被清空() {
    let _guard = serial();
    let (policy, _) = policy_with("cache-invalidate", &["data"]);
    store(&policy, "comfy_list_workflows", &json!({}), "两份工作流", &json!({}));
    assert_eq!(entry_count(), 1);
    assert_eq!(invalidate_all(), 1);
    assert!(lookup(&policy, "comfy_list_workflows", &json!({})).is_none());
    assert_eq!(entry_count(), 0);
}
