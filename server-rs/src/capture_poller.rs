//! 后台捕获轮询器（P4 第 47 步 = 总纲 §10.2 的 **O5 / O6**）。
//!
//! # 为什么必须有它
//!
//! Rust 侧此前只有"端点/工具触发一次"的抓取（`POST /api/capture/poll` 与
//! `comfy_sync_history`）。于是**用户自己在 ComfyUI 界面里跑的东西永远不会被收** ——
//! 表现就是那句经典抱怨："AI 说生成了、画廊里却没有"的另一半。
//! Kotlin 侧对应物是 `ComfyCapture.start()`。
//!
//! # 与 Kotlin 逐条对齐的行为
//!
//! | 行为 | Kotlin | 这里 |
//! | --- | --- | --- |
//! | 启动后**先等 2.5 秒**再跑第一轮 | `delay(2500)` | ✅（避开"后端起好了、ComfyUI 还没好"那几秒） |
//! | 配置里 `enabled=false` 时**空转但不退出** | `delay(5_000)` + `continue` | ✅（用户在设置里打开就立刻生效，不必重启后端） |
//! | 间隔 = `pollSeconds`，夹在 **1..600** | `coerceIn(1, 600)` | ✅（0 或负数会让循环变成忙等） |
//! | **同一时刻只跑一轮** | `pollInFlight.compareAndSet` | ✅（[`CapturePoller::tick`] 的 `in_flight`） |
//! | 成功写 `lastPollAt` 并**清空** `lastError` | `lastPollAt = now; lastError = null` | ✅ |
//! | 失败写 `lastError`（人话，由 `comfy_client::friendly_error` 生成） | `friendlyError(e, url)` | ✅（**不假装成功**：连不上就如实记） |
//! | 读配置失败/没配置 → 当作"停用"空转 | `SettingsRepo.captureConfig` 兜底 | ✅ |
//!
//! ⚠ 一处**刻意不同**：Kotlin 的 `lastPollAt` 是 `Instant`（内存），
//! 这里存的是 **ISO-8601 文本**（与整库的时间列同一个口径，`iso_time::from_system_time`）——
//! 它要经 `/api/capture/status` 出去给界面看，形状与别的可空时间键一致才不会让 Dart 解析出两种东西。
//!
//! ⚠ 另一处**必须记住的边界**：ComfyUI 的 `/history` 是**进程内**的（2026-09-25 实测）。
//! 轮询只在"同一个实例生命周期内"有效 —— 重启 ComfyUI 之后上一次的运行就查不回来了。
//! 所以 [`crate::capture_ingest::poll_once`] 只是**兜底**；`comfy_submit` 必须自带
//! "提交 → 自己等 → 立即入库"（O4）。这条写在 `docs/pitfalls.md` 里，别把兜底当主路。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::capture::{capture_config, CaptureConfig};
use crate::routes::AppState;

/// 启动后第一轮之前的等待（Kotlin `delay(2500)`）。
pub const STARTUP_DELAY_MS: u64 = 2_500;
/// 停用时每一轮的空转间隔（Kotlin `delay(5_000)`）。
pub const DISABLED_INTERVAL_MS: u64 = 5_000;
/// 间隔上下限（Kotlin `coerceIn(1, 600)`）。
pub const MIN_INTERVAL_SECONDS: i64 = 1;
pub const MAX_INTERVAL_SECONDS: i64 = 600;

/// 一轮的结果（给日志与用例看；界面看的是 `/api/capture/status` 里那两个字段）。
#[derive(Debug, Clone, PartialEq)]
pub struct PollTick {
    /// 这一轮真的问了 ComfyUI 吗（`enabled=false` 时空转 → false）。
    pub ran: bool,
    /// `/history` 通了没有。
    pub ok: bool,
    pub checked: i64,
    pub new_runs: i64,
    pub new_media: i64,
    /// 失败时的人话（成功是 None）。
    pub error: Option<String>,
}

/// 轮询器的**内存状态**（与 Kotlin 那两个 `@Volatile` 字段一一对应）。
#[derive(Default)]
pub struct CapturePoller {
    /// 最后一次**成功**轮询的时间（ISO-8601，UTC）。
    last_poll_at: Mutex<Option<String>>,
    /// 最后一次失败的人话；成功会清空（Kotlin `lastError = null`）。
    last_error: Mutex<Option<String>>,
    /// 同一时刻只跑一轮。
    in_flight: AtomicBool,
}

impl CapturePoller {
    pub fn new() -> Self {
        Self::default()
    }

    /// `(lastPollAt, lastError)` —— 两个都可空，`/api/capture/status` 直接用它。
    pub fn snapshot(&self) -> (Option<String>, Option<String>) {
        (
            self.last_poll_at.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            self.last_error.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        )
    }

    pub fn mark_ok(&self, at: String) {
        *self.last_poll_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(at);
        // 成功必须把上一次的错误**清掉**，否则界面会永远挂着一句陈旧的红色提示
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn mark_error(&self, message: impl Into<String>) {
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(message.into());
    }

    /// 跑一轮（**可测**：不发任务、不睡眠，直接把当前配置轮一次）。
    ///
    /// `config` 传 `None` = 读配置失败/停用 → 空转（`ran=false`），与 Kotlin 同。
    pub async fn tick(&self, state: &AppState, config: Option<&CaptureConfig>) -> PollTick {
        let Some(config) = config else {
            return PollTick { ran: false, ok: false, checked: 0, new_runs: 0, new_media: 0, error: None };
        };
        if !config.enabled {
            return PollTick { ran: false, ok: false, checked: 0, new_runs: 0, new_media: 0, error: None };
        }
        // 上一轮还没结束就让开（Kotlin `pollInFlight`）—— 绝不并发收同一次运行
        if self.in_flight.swap(true, Ordering::SeqCst) {
            return PollTick {
                ran: true,
                ok: false,
                checked: 0,
                new_runs: 0,
                new_media: 0,
                error: Some("上一次轮询还没结束".to_string()),
            };
        }
        let result = crate::capture_ingest::poll_once(state, config).await;
        self.in_flight.store(false, Ordering::SeqCst);

        let ok = result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        let num = |key: &str| result.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
        let message = result.get("message").and_then(|v| v.as_str()).map(|s| s.to_string());
        if ok {
            self.mark_ok(crate::iso_time::from_system_time(std::time::SystemTime::now()));
        } else if let Some(message) = message.clone() {
            self.mark_error(message);
        }
        PollTick {
            ran: true,
            ok,
            checked: num("checked"),
            new_runs: num("newRuns"),
            new_media: num("newMedia"),
            error: if ok { None } else { message },
        }
    }

    /// 起后台任务（**只在 `main.rs` 调一次**；测试调 [`CapturePoller::tick`]）。
    ///
    /// 这个循环**永不退出**（除非进程结束）：配置改了、ComfyUI 起停，都由下一轮自己适应 ——
    /// 与 Kotlin 那个 `while (isActive)` 一样，"用户打开开关就立刻生效"是需求。
    pub fn spawn(self: &std::sync::Arc<Self>, state: AppState) {
        let poller = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(STARTUP_DELAY_MS)).await;
            loop {
                let config = capture_config(&state).ok();
                let tick = poller.tick(&state, config.as_ref()).await;
                if tick.ran && (tick.new_runs > 0 || tick.error.is_some()) {
                    // 只在"真的收了东西"或"出错了"时打日志 —— 每 4 秒一行会把日志淹掉
                    eprintln!(
                        "[capture] 轮询：checked={} newRuns={} newMedia={}{}",
                        tick.checked,
                        tick.new_runs,
                        tick.new_media,
                        tick.error.as_deref().map(|e| format!(" error={e}")).unwrap_or_default()
                    );
                }
                let interval = match config {
                    Some(config) if config.enabled => {
                        config.poll_seconds.clamp(MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS)
                    }
                    _ => (DISABLED_INTERVAL_MS / 1000) as i64,
                };
                tokio::time::sleep(std::time::Duration::from_secs(interval.max(1) as u64)).await;
            }
        });
    }
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::CaptureConfig;

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
        AppState::new(crate::db::open_memory().unwrap(), ":memory:", std::env::temp_dir())
    }

    #[tokio::test]
    async fn 停用时空转_不假装轮询过() {
        let poller = CapturePoller::new();
        let state = state();
        let tick = poller.tick(&state, Some(&config(false, "http://127.0.0.1:9", 4))).await;
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
        let tick = poller.tick(&state, Some(&config(true, "http://127.0.0.1:9", 4))).await;
        assert!(tick.ran, "配置是启用的，这一轮真的跑了");
        assert!(!tick.ok);
        let (last_poll_at, last_error) = poller.snapshot();
        assert!(last_poll_at.is_none(), "失败不许写 lastPollAt（Kotlin 只在成功时写）");
        let error = last_error.expect("失败必须留下人话");
        assert!(error.contains("连不上 ComfyUI"), "{error}");
    }

    #[tokio::test]
    async fn 成功会清掉上一次的错误() {
        let poller = CapturePoller::new();
        poller.mark_error("上一次的错误");
        assert!(poller.snapshot().1.is_some());
        poller.mark_ok(crate::iso_time::from_system_time(std::time::SystemTime::now()));
        let (at, error) = poller.snapshot();
        assert!(at.is_some());
        assert!(error.is_none(), "成功必须把陈旧的错误清掉");
    }

    #[test]
    fn 间隔夹在_kotlin_同一档() {
        assert_eq!(0i64.clamp(MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS), 1);
        assert_eq!(9_999i64.clamp(MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS), 600);
        assert_eq!(4i64.clamp(MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS), 4);
    }
}
