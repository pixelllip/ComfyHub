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
//! 2. **本机/内网强制直连**：[`crate::provider::upstream::client_for`] 按目标主机选代理
//!    （公网走代理、`127.0.0.1` 直连）。ComfyUI 永远在本机或内网，这条不遵守的后果是
//!    "打开系统代理之后连不上 ComfyUI"（AGENTS §4 那条实测）。
//! 3. **错误要转换为可读说明**（Kotlin `friendlyError`）：把 `ConnectException` 的类名直接展示，
//!    他只会看到一串 Java 内部名字，而不是"ComfyUI 没在运行？"。

use std::path::PathBuf;

use serde_json::Value;

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
        running_label: running
            .and_then(|items| items.first())
            .and_then(label_of_queue_item),
    })
}

/// `POST {comfy_url}/upload/image`：把一段字节交给 **ComfyUI 自己**写进它的 `input` 目录，
/// 返回它在那边真实可用的**裸文件名**。
///
/// # 必须提供这条路径的依据（「comfy_use_attachment 总是失败」）
///
/// 直写 `input` 目录在真机上会拿到 `拒绝访问 (os error 5)`：后端进程没有那个目录的写权限，
/// 而 **ComfyUI 自己是有的** —— 同一次会话里 MCP 的 `upload_file`（走的就是这条 HTTP）成功。
/// 所以投放/写遮罩**一律先让 ComfyUI 自己写**，直写只当兜底（ComfyUI 没起来时要能用）。
/// 顺带解决另外两件事：ComfyUI 不在本机、或它的 input 不在我们推出来的那个路径时，
/// 直写根本无从谈起 —— 只有这条 HTTP 是永远成立的。
///
/// 手写 multipart 而不是开 `reqwest` 的 `multipart` feature：那个 feature 会多拖一个
/// 依赖（`mime_guess`），而这里要发的形状是固定的四个字段，自己拼更省事也更好断言。
pub async fn upload_image(
    comfy_url: &str,
    filename: &str,
    subfolder: &str,
    bytes: &[u8],
) -> Result<String, String> {
    let base = comfy_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("ComfyUI 地址没配置".to_string());
    }
    // 边界串只要**不与正文撞**即可：用纳秒时间戳，够随机也够可读
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let boundary = format!("----comfyhub{stamp:x}");
    // ⚠ 文件名只放 ASCII 安全字符（`input_filename_for` 已经保证）：multipart 头里的
    // 非 ASCII 要么被服务端按 latin1 解出乱码，要么直接 400
    let safe: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() {
        "image.png".to_string()
    } else {
        safe
    };

    let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 512);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"image\"; filename=\"{safe}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
    push_form_field(&mut body, &boundary, "type", "input");
    push_form_field(&mut body, &boundary, "subfolder", subfolder);
    push_form_field(&mut body, &boundary, "overwrite", "true");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let url = format!("{base}/upload/image");
    let host = host_of(&url);
    let client = crate::provider::upstream::client_for(&host)
        .map_err(|e| format!("HTTP 客户端创建失败：{}", e.human()))?;
    let response = client
        .post(&url)
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("Accept", "application/json")
        // 几十 MB 的图在慢盘上要几十秒；20 秒会把正常情况判成失败
        .timeout(std::time::Duration::from_secs(120))
        .body(body)
        .send()
        .await
        .map_err(|e| friendly_error(&e, &url))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("读响应失败：{e}"))?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {}：{}",
            status.as_u16(),
            take_chars(&text, 300)
        ));
    }
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("响应不是 JSON：{e}（原文：{}）", take_chars(&text, 200)))?;
    let name = parsed
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("上传响应里没有 name：{}", take_chars(&text, 200)))?;
    // 返回给模型的必须是 **LoadImage 认得的那串**：ComfyUI 把 subfolder 与原文件名分开回，
    // 而 `LoadImage.image` 在有子目录时要写成 `子目录/文件名`。我们恒用空 subfolder，
    // 但这条兜住"ComfyUI 自己给了子目录"的情况。
    let sub = parsed
        .get("subfolder")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    Ok(if sub.is_empty() {
        name.to_string()
    } else {
        format!("{sub}/{name}")
    })
}

/// 往 multipart 体里追加一个普通字段（`upload_image` 用；抽成函数是为了能同时借用 `body`）。
fn push_form_field(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(value.as_bytes());
    body.extend_from_slice(b"\r\n");
}

/// 一次 POST JSON（`POST /prompt`）：超时 **30 秒**（Kotlin `postPrompt` 同）。
///
/// ⚠ 非 2xx 时**把响应体带进错误**：ComfyUI 的 400 会说清哪个节点的哪个参数不对，
/// 那段话正是模型改图要用的（只说 "HTTP 400" 等于什么都没说）。
pub async fn post_json(url: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let host = host_of(url);
    let client = crate::provider::upstream::client_for(&host)
        .map_err(|e| format!("HTTP 客户端创建失败：{}", e.human()))?;
    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| friendly_error(&e, url))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("读响应失败：{e}"))?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {}：{}",
            status.as_u16(),
            take_chars(&text, 600)
        ));
    }
    serde_json::from_str(&text)
        .map_err(|e| format!("响应不是 JSON：{e}（原文：{}）", take_chars(&text, 300)))
}

/// 按**字符**截断（别把 UTF-8 字符劈成两半）。
fn take_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 一次 GET：超时 20 秒（Kotlin `fetchJson` 同），非 2xx 当错误。
///
/// `pub`：捕获轮询（`/history`）也走它 —— ComfyUI 的地址与代理纪律只有这一处实现。
pub async fn get_json(url: &str) -> Result<serde_json::Value, String> {
    get_json_with_timeout(url, 20).await
}

/// 同上，但**超时自定**：探测实例 input 目录那条路要一个短超时
/// （实例没起来时"连不上"是瞬间的，但地址写错时不该把投放卡 20 秒）。
pub async fn get_json_with_timeout(
    url: &str,
    timeout_secs: u64,
) -> Result<serde_json::Value, String> {
    let host = host_of(url);
    let client = crate::provider::upstream::client_for(&host)
        .map_err(|e| format!("HTTP 客户端创建失败：{}", e.human()))?;
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .send()
        .await
        .map_err(|e| friendly_error(&e, url))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    let text = response
        .text()
        .await
        .map_err(|e| format!("读响应失败：{e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("响应不是 JSON：{e}"))
}

// ---------------------------------------------------------------------------
//  问实例："你读的是哪个 input 目录？"（2026-09-30 那个 400 的解药）
// ---------------------------------------------------------------------------

/// 探测实例 input 目录的超时（秒）。短一点：**问不到就当没问**（回落候选顺序），
/// 不许因为一次探测把投放卡住。
const PROBE_TIMEOUT_SECS: u64 = 5;

/// 最多拿实例名单里的多少个名字去比对（input 目录可能有几千个文件，没必要全比）。
const PROBE_MAX_NAMES: usize = 200;

/// 实例"看得见"的 input 文件名单（`GET /object_info/LoadImage` 里 `image` 的候选值）。
///
/// 这是**实例自己报的** —— 它排除了"配置里写了什么"，是"它到底读哪儿"的唯一权威答案。
pub async fn instance_input_names(comfy_url: &str) -> Result<Vec<String>, String> {
    let base = comfy_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("ComfyUI 地址没配置".to_string());
    }
    let body =
        get_json_with_timeout(&format!("{base}/object_info/LoadImage"), PROBE_TIMEOUT_SECS).await?;
    let names = body
        .get("LoadImage")
        .and_then(|node| node.get("input"))
        .and_then(|input| input.get("required"))
        .and_then(|required| required.get("image"))
        .and_then(Value::as_array)
        .and_then(|spec| spec.first())
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
    names.ok_or_else(|| {
        "object_info 里没有 LoadImage.image 的候选名单（这个实例不认识 LoadImage？）".to_string()
    })
}

/// **哪一个候选目录装着实例报出来的那些文件** —— 也就是"实例真正在读的那个 input"。
///
/// 判据是**覆盖面**：实例的名单就是它 input 目录的一份完整枚举，所以覆盖它最多的那个候选
/// 就是它（两个目录里都有的公共文件，比如 ComfyUI 自带的 `example.png`，只算一票，
/// 分不出高下）。
///
/// ⚠ **并列第一就 `None`**（宁可回落到候选顺序，也不猜）：猜错的表现是提交时
/// `400 Invalid image file`，比"没探测到"更难查。
pub fn dir_owning(candidates: &[PathBuf], names: &[String]) -> Option<PathBuf> {
    if names.is_empty() {
        return None;
    }
    let wanted: Vec<String> = names
        .iter()
        .take(PROBE_MAX_NAMES)
        // 子目录形式（`sub/a.png`）按文件名比 —— 我们投放恒用空 subfolder，
        // 但实例那边给出来的可能是带子目录的写法
        .map(|name| name.rsplit('/').next().unwrap_or(name).to_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    let mut best: Option<(PathBuf, usize)> = None;
    let mut tied = false;
    for candidate in candidates {
        let Ok(entries) = std::fs::read_dir(candidate) else {
            continue;
        };
        let present: std::collections::HashSet<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_lowercase())
            .collect();
        let hits = wanted.iter().filter(|name| present.contains(*name)).count();
        if hits == 0 {
            continue;
        }
        match &best {
            Some((_, best_hits)) if hits == *best_hits => tied = true,
            Some((_, best_hits)) if hits < *best_hits => {}
            _ => {
                best = Some((candidate.clone(), hits));
                tied = false;
            }
        }
    }
    match best {
        Some((dir, _)) if !tied => Some(dir),
        _ => None,
    }
}

/// [`dir_owning`] 的线上版：问一次实例，答话就用它，问不到就 `None`（不报错 —— 见 [`dir_owning`]）。
pub async fn instance_input_dir(comfy_url: &str, candidates: &[PathBuf]) -> Option<PathBuf> {
    let names = instance_input_names(comfy_url).await.ok()?;
    dir_owning(candidates, &names)
}

/// 取一段二进制（产物下载兜底 `GET /view`）。
///
/// `timeout_secs` 由调用方给：`/view` 可能是一段几百 MB 的视频，用查询端点那 20 秒会
/// 把慢盘上的大文件判成失败（Kotlin 那边给的是 5 分钟）。
pub async fn get_bytes(url: &str, timeout_secs: u64) -> Result<Vec<u8>, String> {
    let host = host_of(url);
    let client = crate::provider::upstream::client_for(&host)
        .map_err(|e| format!("HTTP 客户端创建失败：{}", e.human()))?;
    let response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .send()
        .await
        .map_err(|e| friendly_error(&e, url))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    response
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("读响应失败：{e}"))
}

/// URL → 主机名（给 [`crate::infra::outbound::route`] 判"公网还是本机"用）。
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

/// 网络异常 → 可直接阅读的一句说明（Kotlin `friendlyError` 的对应物）。
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
        let mut cursor: Option<&(dyn std::error::Error + 'static)> =
            std::error::Error::source(self);
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
/// 标题位于 `extra_data.extra_pnginfo.workflow.extra.title`（在 ComfyUI 中为工作流设置的名称）。
/// 取不到就 `None` —— 界面宁可只说"正在生成…"，也不要编一个名字出来。
fn label_of_queue_item(item: &serde_json::Value) -> Option<String> {
    let row = item.as_array()?;
    let extra = row.iter().find_map(|entry| {
        let obj = entry.as_object()?;
        obj.contains_key("extra_pnginfo").then_some(obj)
    })?;
    let workflow = extra
        .get("extra_pnginfo")?
        .as_object()?
        .get("workflow")?
        .as_object()?;
    let title = workflow.get("extra")?.as_object()?.get("title")?.as_str()?;
    let trimmed = title.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests;
