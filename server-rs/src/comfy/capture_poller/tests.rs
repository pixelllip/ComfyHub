//! `capture_poller` 的单元测试（主体代码在 [`capture_poller`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;
use crate::comfy::capture::CaptureConfig;

fn config(enabled: bool, url: &str, seconds: i64) -> CaptureConfig {
    CaptureConfig {
        enabled,
        comfy_url: url.to_string(),
        output_dir: None,
        poll_seconds: seconds,
        auto_tag: "ComfyUI".to_string(),
        max_per_poll: 20,
        download_fallback: true,
    }
}

fn state() -> AppState {
    AppState::new(
        crate::infra::db::open_memory().unwrap(),
        ":memory:",
        crate::infra::testpaths::scratch("media"),
    )
}

#[tokio::test]
async fn 停用时空转_不假装轮询过() {
    let poller = CapturePoller::new();
    let state = state();
    let tick = poller
        .tick(&state, Some(&config(false, "http://127.0.0.1:9", 4)))
        .await;
    assert!(!tick.ran);
    assert_eq!(poller.snapshot(), (None, None), "停用时两个字段都该是空的");
    // 读不到配置（None）也是同一种空转
    let tick = poller.tick(&state, None).await;
    assert!(!tick.ran && tick.error.is_none());
}

#[tokio::test]
async fn 连不上时如实记_last_error_而不是假装成功() {
    let poller = CapturePoller::new();
    let state = state();
    // 127.0.0.1:9（discard 端口）上不会有人听
    let tick = poller
        .tick(&state, Some(&config(true, "http://127.0.0.1:9", 4)))
        .await;
    assert!(tick.ran, "配置是启用的，这一轮真的跑了");
    assert!(!tick.ok);
    let (last_poll_at, last_error) = poller.snapshot();
    assert!(
        last_poll_at.is_none(),
        "失败不许写 lastPollAt（Kotlin 只在成功时写）"
    );
    let error = last_error.expect("失败必须留下人话");
    assert!(error.contains("连不上 ComfyUI"), "{error}");
}

#[tokio::test]
async fn 成功会清掉上一次的错误() {
    let poller = CapturePoller::new();
    poller.mark_error("上一次的错误");
    assert!(poller.snapshot().1.is_some());
    poller.mark_ok(crate::infra::iso_time::from_system_time(
        std::time::SystemTime::now(),
    ));
    let (at, error) = poller.snapshot();
    assert!(at.is_some());
    assert!(error.is_none(), "成功必须把陈旧的错误清掉");
}

#[test]
fn 间隔夹在_kotlin_同一档() {
    assert_eq!(0i64.clamp(MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS), 1);
    assert_eq!(
        9_999i64.clamp(MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS),
        600
    );
    assert_eq!(4i64.clamp(MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS), 4);
}
