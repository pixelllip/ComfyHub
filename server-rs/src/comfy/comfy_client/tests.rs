//! `comfy_client` 的单元测试（主体代码在 [`comfy_client`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn host_of_handles_ports_and_ipv6() {
    assert_eq!(host_of("http://127.0.0.1:8188/queue"), "127.0.0.1");
    assert_eq!(host_of("http://localhost:8188"), "localhost");
    assert_eq!(host_of("https://comfy.example.com/queue"), "comfy.example.com");
    assert_eq!(host_of("http://[::1]:8188/queue"), "[::1]");
    assert_eq!(host_of("http://user:pw@192.168.1.9:8188/queue"), "192.168.1.9");
}

/// 标题藏在 `extra_data.extra_pnginfo.workflow.extra.title`（Kotlin 逐字同）。
#[test]
fn queue_label_reads_the_workflow_title_only() {
    let with_title = serde_json::json!([
        1,
        "abc",
        {},
        { "extra_pnginfo": { "workflow": { "extra": { "title": "krea2 出图" } } } },
        {}
    ]);
    assert_eq!(label_of_queue_item(&with_title).as_deref(), Some("krea2 出图"));
    // 没有标题 / 空标题 / 形状不对 → None（**不许编一个名字**）
    let no_title = serde_json::json!([1, "abc", {}, { "extra_pnginfo": { "workflow": {} } }, {}]);
    assert_eq!(label_of_queue_item(&no_title), None);
    let blank = serde_json::json!([
        1,
        "abc",
        {},
        { "extra_pnginfo": { "workflow": { "extra": { "title": "   " } } } },
        {}
    ]);
    assert_eq!(label_of_queue_item(&blank), None);
    assert_eq!(label_of_queue_item(&serde_json::json!({})), None);
}

/// **哪一个候选目录装着实例报出来的那些文件** —— 也就是实例真正在读的那个 input。
///
/// 用 2026-09-30 那台机器的真实形状当用例：安装目录那份 input 有 18 个文件、
/// 共享那份有 92 个，而实例报出来的名单是**共享**那份（它的 argv 里两条 input 参数、
/// argparse 取后写的 `--input-directory <共享>`）。
#[test]
fn dir_owning_picks_the_input_the_instance_actually_reads() {
    let root = crate::infra::testpaths::scratch("dir-owning");
    let install = root.join("ComfyUI-Installs").join("ComfyUI").join("input");
    let shared = root.join("ComfyUI-Shared").join("input");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::create_dir_all(&shared).unwrap();
    // 两边都有的公共文件（ComfyUI 自带的 example.png 就是这种）
    for dir in [&install, &shared] {
        std::fs::write(dir.join("example.png"), "x").unwrap();
    }
    // 只有共享那份有的（实例名单里绝大多数是这一类）
    for name in ["a-00000000.jpg", "b-00000000.jpg", "c-00000000.jpg"] {
        std::fs::write(shared.join(name), "x").unwrap();
    }
    // 只有安装目录那份有的（我们投错地方时写进去的就是这种）
    std::fs::write(install.join("comfyhub_t2i_anima_00024_-m1.png"), "x").unwrap();

    let names: Vec<String> =
        ["example.png", "a-00000000.jpg", "b-00000000.jpg", "c-00000000.jpg"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    let picked = dir_owning(&[install.clone(), shared.clone()], &names);
    assert_eq!(picked, Some(shared.clone()), "谁覆盖得多就是谁 —— 公共文件只算一票");

    // 并列第一（两边都只装着公共那份）→ **None**：宁可回落到候选顺序，也不猜
    assert_eq!(dir_owning(&[install.clone(), shared], &["example.png".to_string()]), None);
    // 名单是空的 / 候选里一个都装不下 → None
    assert_eq!(dir_owning(std::slice::from_ref(&install), &[]), None);
    assert_eq!(dir_owning(&[install], &["nope.png".to_string()]), None);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn empty_url_is_reported_not_panicking() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let err = runtime.block_on(queue("   ")).unwrap_err();
    assert!(err.contains("没配置"), "{err}");
}
