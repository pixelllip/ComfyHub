//! ComfyUI 的 HTTP 客户端（P4 第 42 步）—— 先只搬"查询"那一半。
//!
//! # 现在有什么
//!
//! - [`queue`]：`GET {comfyUrl}/queue` → 运行中 / 等待中条数与**队首的任务名**
//!   （Kotlin `ComfyCapture.refreshQueue` + `labelOfQueueItem` 的逐行对照）。
//!
//! 还没搬：`/history`（捕获轮询要用，归 `capture::poll_once` 那一步）、
//! `/prompt`（提交）、`/object_info`（转换与模型清单）、`/system_stats`（环境对账）。
//! **没搬的绝不假装**：调用方要它们时会拿到明确的"还没搬"错误。
//!
//! # 三条纪律
//!
//! 1. **地址由应用配置决定，不接受任意 URL**（工具描述里就写着这句）：调用方传进来的
//!    永远是配置里那一份，不接受模型给的地址 —— 否则这就是一个 SSRF 入口。
//! 2. **本机/内网强制直连**：[`crate::upstream::client_for`] 按目标主机选代理
//!    （公网走代理、`127.0.0.1` 直连）。ComfyUI 永远在本机或内网，这条不遵守的后果是
//!    "打开系统代理之后连不上 ComfyUI"（AGENTS §4 那条实测）。
//! 3. **错误要翻成人话**（Kotlin `friendlyError`）：把 `ConnectException` 的类名丢给用户，
//!    他只会看到一串 Java 内部名字，而不是"ComfyUI 没在运行？"。

/// 一次队列快照（Kotlin `refreshQueue` 的三个字段）。
#[derive(Debug, Clone, PartialEq)]
pub struct QueueSnapshot {
    pub running: usize,
    pub pending: usize,
    /// 队首任务在 ComfyUI 里的标题（取不到就是 `None`：**宁可只说"正在生成…"也不编一个**）
    pub running_label: Option<String>,
}

/// `GET {comfy_url}/queue`。
pub async fn queue(comfy_url: &str) -> Result<QueueSnapshot, String> {
    let base = comfy_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("ComfyUI 地址没配置".to_string());
    }
    let body = get_json(&format!("{base}/queue")).await?;
    let running = body.get("queue_running").and_then(|v| v.as_array());
    let pending = body.get("queue_pending").and_then(|v| v.as_array());
    Ok(QueueSnapshot {
        running: running.map(Vec::len).unwrap_or(0),
        pending: pending.map(Vec::len).unwrap_or(0),
        running_label: running.and_then(|items| items.first()).and_then(label_of_queue_item),
    })
}

/// 一次 GET：超时 20 秒（Kotlin `fetchJson` 同），非 2xx 当错误。
async fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let host = host_of(url);
    let client = crate::upstream::client_for(&host)
        .map_err(|e| format!("HTTP 客户端创建失败：{}", e.human()))?;
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| friendly_error(&e, url))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    let text = response.text().await.map_err(|e| format!("读响应失败：{e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("响应不是 JSON：{e}"))
}

/// URL → 主机名（给 [`crate::outbound::route`] 判"公网还是本机"用）。
fn host_of(url: &str) -> String {
    let rest = url.split("//").nth(1).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // 去掉 user:pass@ 前缀与端口（IPv6 的 [::1]:8188 要保住方括号里的内容）
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(end) = authority.find(']') {
        return authority[..=end].to_string();
    }
    authority.split(':').next().unwrap_or(authority).to_string()
}

/// 网络异常 → 用户能看懂的一句话（Kotlin `friendlyError` 的对应物）。
///
/// ⚠ reqwest 把"连接被拒 / DNS 失败 / 超时"都折进一个 `reqwest::Error`，还**不保证**
/// 底层 `io::Error` 一定挂着（超时就没有）—— 所以这里先问 `is_timeout`/`is_connect`，
/// 再退到底层 IO 的 `kind()`，最后才用原文。顺序反过来的话，"连接被拒绝"会被报成原文那串
/// 内部英文，而那正是这条函数要消灭的东西。
pub fn friendly_error(error: &reqwest::Error, comfy_url: &str) -> String {
    if error.is_timeout() {
        return "连接超时".to_string();
    }
    if let Some(io) = error.source_io() {
        return match io.kind() {
            std::io::ErrorKind::ConnectionRefused => "连接被拒绝 —— ComfyUI 没在运行？".to_string(),
            std::io::ErrorKind::TimedOut => "读取超时".to_string(),
            std::io::ErrorKind::NotFound => format!("地址无法解析：{comfy_url}"),
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted => {
                "连接被重置 —— ComfyUI 刚重启？".to_string()
            }
            _ => {
                let text = io.to_string();
                if text.trim().is_empty() {
                    format!("网络错误（{}）", io.kind())
                } else {
                    text
                }
            }
        };
    }
    if error.is_connect() {
        return "连接被拒绝 —— ComfyUI 没在运行？".to_string();
    }
    let text = error.to_string();
    if text.trim().is_empty() {
        "未知网络错误".to_string()
    } else {
        text
    }
}

/// 取 `io::Error`（`reqwest::Error` 的 source 链里那一层）。
trait SourceIo {
    fn source_io(&self) -> Option<&std::io::Error>;
}

impl SourceIo for reqwest::Error {
    fn source_io(&self) -> Option<&std::io::Error> {
        let mut cursor: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(self);
        while let Some(current) = cursor {
            if let Some(io) = current.downcast_ref::<std::io::Error>() {
                return Some(io);
            }
            cursor = current.source();
        }
        None
    }
}

/// 队列项是 `[number, prompt_id, graph, extra_data, outputs]`：
/// 标题藏在 `extra_data.extra_pnginfo.workflow.extra.title`（用户在 ComfyUI 里给工作流起的名字）。
/// 取不到就 `None` —— 界面宁可只说"正在生成…"，也不要编一个名字出来。
fn label_of_queue_item(item: &serde_json::Value) -> Option<String> {
    let row = item.as_array()?;
    let extra = row.iter().find_map(|entry| {
        let obj = entry.as_object()?;
        obj.contains_key("extra_pnginfo").then_some(obj)
    })?;
    let workflow = extra.get("extra_pnginfo")?.as_object()?.get("workflow")?.as_object()?;
    let title = workflow.get("extra")?.as_object()?.get("title")?.as_str()?;
    let trimmed = title.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn empty_url_is_reported_not_panicking() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let err = runtime.block_on(queue("   ")).unwrap_err();
        assert!(err.contains("没配置"), "{err}");
    }
}
