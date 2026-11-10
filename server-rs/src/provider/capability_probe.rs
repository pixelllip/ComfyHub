//! 思考档位的**实测校准**（`POST /api/ai/capabilities/probe`）。
//!
//! 逐条对照 Kotlin `ThinkingProbe.kt` + `AiUpstream.probeThinkingLadder`（那两份是行为真源）；
//! 落点是 `<storage>\ai\capabilities\custom.json`（**不是**库里的 `ai_models` 行）——
//! 目录是"模型能力从哪来"的真源，覆盖层优先级最高，改动立刻生效、不用重启也不用发版。
//!
//! # 为什么需要它
//!
//! 模态与"是否支持思考"有数据集（models.dev）可查，但**档位阶梯的线上表达**没有标准接口 ——
//! 而且聚合站经常整体忽略思考参数（2026-09-20 实测）。唯一可靠的来源是
//! 真发极小请求看网关认不认（`reasoning_effort=<档位>`，见 `protocol.rs` 的方言表）。
//!
//! # 六条纪律（每一条都有对应的用例）
//!
//!  1. **默认关**：`probe-toggle` 没打开 → **一个上游请求都不发**（`transport` 是注入的，
//!     用例直接断言调用次数为 0）。这句话必须如实说清"它会花钱"；
//!  2. **只打推理模型**：`enabled && reasoning`（判据沿用模型目录那份，别在这里另发明一套）；
//!     `providerId` / `modelId` 可缺省（缺省 = 全部启用的 Provider / 该 Provider 全部推理模型，
//!     每个 Provider 最多 [`MAX_MODELS_PER_PROVIDER`] 个 —— 防手滑全库烧钱，与 Kotlin 同值）；
//!  3. **请求极小 + 逐模型独立**：短 prompt + `max_tokens: 256` + 每请求 [`PROBE_TIMEOUT_SECONDS`]
//!     超时上限；一个模型失败/超时**不许把整批拖死**，结果逐条如实（哪几个试成了、为什么失败）；
//!  4. **写回不许碰别人的行**：只动这一个模型的 `thinkingEfforts`（必要时 `thinkingFormat`），
//!     **别的条目一个字节都不变**（见 [`splice_entry`]：已有条目按字节定位、就地替换）；
//!     写盘前先读取（`custom.json` 为手动编写，**损坏时需如实报告、不得覆盖为空文件**），
//!     落盘是临时文件 + rename（原子）；
//!  5. **不猜**：网关一个档位都不认就如实 `ok:false` + `acceptedTop: null`，**绝不发明**档位；
//!     `ladder` 里只放**真试过**的那些（从被接受的那一档往下）；
//!  6. **测试不出网**：真正的网络动作全在 [`Transport`] 后面，用例喂假 transport。

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::ai::ai::AiProviderRow;
use crate::provider::capability_catalog::{OverlayEntry, CUSTOM_FILE};
use crate::web::routes::{ApiError, AppState};

/// 单个 Provider 一次最多校准多少个模型（`ThinkingProbe.MAX_MODELS_PER_PROVIDER`，两边必须同值）。
pub const MAX_MODELS_PER_PROVIDER: usize = 8;

/// 一次探测请求的超时上限（Kotlin `probeThinkingLadder(..., timeoutSeconds = 30L)`）。
///
/// ⚠ **必须有上限**：没有它，一个卡住的网关会把整批校准吊死（一次请求半分钟是上限，
/// 而每个 Provider 最多 8 个模型、每个模型一次基准 + 若干档位）。
pub const PROBE_TIMEOUT_SECONDS: u64 = 30;

/// 极小请求的输出上限（Kotlin `probeBody` 里的 `256`）。
pub const PROBE_MAX_TOKENS: i64 = 256;

/// 极小请求的正文（Kotlin `probeBody` 里写死的 `"ping"`）。
pub const PROBE_PROMPT: &str = "ping";

/// 档位阶梯（从低到高；`ThinkingProbe.EFFORT_ORDER` 的逐字对应物）。
///
/// ⚠ 顺序**就是语义**：探测从**最高档往下**试，写回的阶梯是"被接受的那档及其以下" ——
/// 把它排序成别的样子，写回的表就会把网关拒绝的高档也写进去。
const EFFORT_ORDER: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// 档位在 [`EFFORT_ORDER`] 里的位置（不认识的等级排最前，绝不会被当成"高档"优先试）。
fn effort_rank(level: &str) -> usize {
    EFFORT_ORDER.iter().position(|l| *l == level).unwrap_or(0)
}

// ---------------------------------------------------------------------------
//  可注入的传输层（纪律 6：用例一条网都不出）
// ---------------------------------------------------------------------------

/// 一次 `POST` 的结果。`status == None` = 根本没连上（超时 / DNS / 拒绝）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResponse {
    pub status: Option<u16>,
    pub body: String,
    /// 连不上时的原因（已脱敏、已截断）；连上了就是空串
    pub detail: String,
}

impl ProbeResponse {
    /// 2xx（用例里的"网关认了"）。
    #[cfg(test)]
    fn ok(status: u16) -> Self {
        Self {
            status: Some(status),
            body: String::new(),
            detail: String::new(),
        }
    }

    fn failed(detail: impl Into<String>) -> Self {
        Self {
            status: None,
            body: String::new(),
            detail: detail.into(),
        }
    }
}

/// 发一次极小请求的抽象。**唯一的网络出口**：用例注入假实现就能断言"到底发了几次"。
#[allow(async_fn_in_trait)]
pub trait Transport {
    async fn post(
        &self,
        url: &str,
        api: &str,
        secret: Option<&str>,
        trust: &str,
        body: &str,
        timeout_seconds: u64,
    ) -> ProbeResponse;
}

/// 真的发请求（复用 `provider_probe` 那套 SSRF 闸门 + 代理选择 + 密钥脱敏）。
pub struct HttpTransport;

impl HttpTransport {
    /// 发一次 `POST`（Kotlin `probePost` 的逐条对应物）。
    ///
    /// 三条照抄 Kotlin 的地方：
    ///  1. 出网前先过 [`crate::web::endpoint_guard::assert_allowed`]（SSRF 闸门，任何档位都不许松）；
    ///  2. 正文**先脱敏再截断**（有的网关把请求头原样回显）；
    ///  3. 超时**单独设**，不吃客户端默认那 600 秒（那是流式会话的上限）。
    async fn send(
        url: &str,
        api: &str,
        secret: Option<&str>,
        trust: &str,
        body: &str,
        timeout_seconds: u64,
    ) -> ProbeResponse {
        let Some(host) = crate::provider::upstream::host_of(url) else {
            return ProbeResponse::failed("URL 无法解析出主机名");
        };
        if let Err(message) = crate::web::endpoint_guard::assert_allowed(&host, trust) {
            return ProbeResponse::failed(message);
        }
        let client = match crate::provider::upstream::client_for(&host) {
            Ok(client) => client,
            Err(error) => return ProbeResponse::failed(error.human().to_string()),
        };
        let Some(parsed) = crate::provider::protocol::Api::parse(api) else {
            return ProbeResponse::failed(format!("未知协议：{api}"));
        };
        let mut builder = client
            .post(url)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .timeout(std::time::Duration::from_secs(timeout_seconds))
            .body(body.to_string());
        for (name, value) in crate::provider::upstream::auth_header(parsed, secret) {
            builder = builder.header(name, value);
        }
        match builder.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let text = response.text().await.unwrap_or_default();
                ProbeResponse {
                    status: Some(status),
                    body: crate::provider::upstream::redact(&text, secret),
                    detail: String::new(),
                }
            }
            Err(error) => {
                if error.is_timeout() {
                    ProbeResponse::failed(format!("连接超时（{timeout_seconds}秒）"))
                } else {
                    ProbeResponse::failed(format!("无法连接：{error}"))
                }
            }
        }
    }
}

impl Transport for HttpTransport {
    async fn post(
        &self,
        url: &str,
        api: &str,
        secret: Option<&str>,
        trust: &str,
        body: &str,
        timeout_seconds: u64,
    ) -> ProbeResponse {
        Self::send(url, api, secret, trust, body, timeout_seconds).await
    }
}

// ---------------------------------------------------------------------------
//  极小请求与其解释（纯函数，离线可测）
// ---------------------------------------------------------------------------

/// 极小请求的正文（Kotlin `probeBody`）：
/// `{"model":…,"messages":[{"role":"user","content":"ping"}],"max_tokens":256[,"reasoning_effort":"high"]}`。
///
/// 键顺序与 Kotlin 的 `buildJsonObject` 一致（`model` → `messages` → 输出上限 → 档位），
/// 中间件/网关的日志与快照对比才不会每次都"看起来变了"。
pub fn probe_body(model_id: &str, effort: Option<&str>, token_field: &str) -> String {
    let mut root = Map::new();
    root.insert("model".into(), Value::String(model_id.to_string()));
    root.insert(
        "messages".into(),
        Value::Array(vec![
            serde_json::json!({ "role": "user", "content": PROBE_PROMPT }),
        ]),
    );
    root.insert(token_field.to_string(), Value::from(PROBE_MAX_TOKENS));
    if let Some(effort) = effort {
        root.insert("reasoning_effort".into(), Value::String(effort.to_string()));
    }
    Value::Object(root).to_string()
}

/// chat 端点候选（Kotlin `chatUrlCandidates`）：Base URL 填不填 `/v1` 两种写法都试。
pub fn chat_url_candidates(base_url: &str) -> Vec<String> {
    let base = base_url.trim().trim_end_matches('/');
    let has_v1 = base.ends_with("/v1");
    let stripped = if has_v1 {
        base.trim_end_matches("/v1").to_string()
    } else {
        format!("{base}/v1")
    };
    let mut out = vec![format!("{base}/chat/completions")];
    let alternate = format!("{stripped}/chat/completions");
    if !out.contains(&alternate) {
        out.push(alternate);
    }
    out
}

/// 要试的档位（从**最高档往下**）。
///
/// 模型声明了等级表就只试表内的（`off` 永远不试 —— 它是"关掉"，不是"网关认不认的档位"）；
/// 没声明就退到除 `off` 外的整条阶梯。
pub fn rungs_for(declared: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut rungs: Vec<(String, String)> = declared
        .iter()
        .filter(|(level, _)| level.as_str() != "off")
        .map(|(level, wire)| (level.clone(), wire.clone()))
        .collect();
    if rungs.is_empty() {
        // ⚠ `.copied()` 是**必须的**：`EFFORT_ORDER.iter()` 的元素是 `&&str`，
        // 而 `filter` 的闭包参数又是元素的引用（`&&&str`）。少一层 `*` 就会把
        // `&&str` 与 `&str` 拿去比 —— 那种写法**能编过**（有 `PartialEq`），
        // 断言却是恒真/恒假的，某个档位悄悄溜过去也不会红。
        // 这里把它一次消干净：元素是 `&str`、闭包参数是 `&&str`，`*level` 就是 `&str`。
        let mut fallback: Vec<(String, String)> = EFFORT_ORDER
            .iter()
            .copied()
            .filter(|level| *level != "off")
            .map(|level| (level.to_string(), level.to_string()))
            .collect();
        // 探测方向是**从最高档往下**，而 [`EFFORT_ORDER`] 是低→高的声明顺序
        fallback.reverse();
        return fallback;
    }
    rungs.sort_by_key(|rung| std::cmp::Reverse(effort_rank(&rung.0)));
    rungs
}

/// 写回用的阶梯：`off → none` + **被接受的那一档及其以下**。
///
/// 被拒的高档**绝不写回**（那是"我们猜的"，而这次探测的全部意义就是"不猜"）。
pub fn ladder_from(rungs: &[(String, String)], accepted_index: usize) -> BTreeMap<String, String> {
    let mut ladder: BTreeMap<String, String> = BTreeMap::new();
    ladder.insert("off".to_string(), "none".to_string());
    for (level, wire) in rungs.iter().skip(accepted_index) {
        ladder.insert(level.clone(), wire.clone());
    }
    ladder
}

/// 这个 4xx 是不是"思考参数的问题"（Kotlin 那三条 `contains` 的逐字对应物）。
///
/// 判据的重要之处在于**它决定降不降档**：错误文案里没有 effort / reasoning / thinking
/// 的 400（比如"模型不存在"）再降一档只会拿到同样的错，必须**当场如实中止**。
pub fn mentions_thinking(detail: &str) -> bool {
    let lower = detail.to_lowercase();
    lower.contains("effort") || lower.contains("reasoning") || lower.contains("thinking")
}

/// 一次探测的结论（Kotlin `ThinkingProbeOutcome`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LadderOutcome {
    /// 网关**接受**的最高档（`None` = 一个都没认）
    pub accepted_top: Option<String>,
    /// 建议写回的表；`None` = 不该写（失败 / 一个都没认）
    pub ladder: Option<BTreeMap<String, String>>,
    pub message: String,
    /// 真正发出去的请求数（基准 + 试过的档位）—— 如实回报
    pub attempts: usize,
}

impl LadderOutcome {
    fn failed(message: impl Into<String>, attempts: usize) -> Self {
        Self {
            accepted_top: None,
            ladder: None,
            message: message.into(),
            attempts,
        }
    }
}

/// 这一次响应是 2xx 吗（**连不上（`status == None`）一律不算**）。
fn is_success(response: &ProbeResponse) -> bool {
    matches!(response.status, Some(code) if (200..300).contains(&code))
}

/// 从最高档往下试，返回网关实际接受的档位。**纯逻辑 + 注入的 transport**（用例不出网）。
///
/// 顺序照抄 Kotlin：先找一个能用的 chat 端点（404 = 拼写不对就换候选，其它错误就认这个端点），
/// 再发一次**不带思考参数的基准请求**（连基准都过不了就别下结论），最后才逐档试。
pub async fn run_ladder<T: Transport>(
    transport: &T,
    base_url: &str,
    api: &str,
    model_id: &str,
    secret: Option<&str>,
    trust: &str,
    rungs: &[(String, String)],
) -> LadderOutcome {
    // ① 找可用的 chat 端点
    let mut endpoint: Option<String> = None;
    let mut last: Option<ProbeResponse> = None;
    let mut token_field = "max_tokens";
    let mut attempts = 0usize;
    for url in chat_url_candidates(base_url) {
        attempts += 1;
        let response = transport
            .post(
                &url,
                api,
                secret,
                trust,
                &probe_body(model_id, None, token_field),
                PROBE_TIMEOUT_SECONDS,
            )
            .await;
        // 每个探测请求都计为一次尝试（连接失败也算 —— 需显示"实际发送次数"）；
        // 只有 404 才换下一个候选（那是"地址拼错了"），其余一律认这个端点。
        let is404 = response.status == Some(404);
        last = Some(response);
        if !is404 {
            endpoint = Some(url);
            break;
        }
    }
    let Some(endpoint) = endpoint else {
        let reason = describe(last.as_ref(), "没有候选地址");
        return LadderOutcome::failed(format!("找不到可用的 chat 端点：{reason}"), attempts);
    };
    let mut baseline = last.expect("有 endpoint 就一定有响应");

    // ② 有些严格网关只认 `max_completion_tokens`：错误文案点到了 `max_tokens` 就换字段重试一次
    if !is_success(&baseline)
        && baseline.status == Some(400)
        && baseline.body.to_lowercase().contains("max_tokens")
    {
        token_field = "max_completion_tokens";
        attempts += 1;
        baseline = transport
            .post(
                &endpoint,
                api,
                secret,
                trust,
                &probe_body(model_id, None, token_field),
                PROBE_TIMEOUT_SECONDS,
            )
            .await;
    }
    if !is_success(&baseline) {
        return LadderOutcome::failed(
            format!(
                "基准请求（不带思考参数）失败：{}",
                describe(Some(&baseline), "")
            ),
            attempts,
        );
    }
    if rungs.is_empty() {
        return LadderOutcome::failed("模型没有可校准的思考档位", attempts);
    }

    // ③ 逐档试（从最高档往下）。
    //
    // ⚠ 请求里**只带 `reasoning_effort`**：与 Kotlin 完全一致（`probeBody` 里就这一个思考字段），
    // 所以一个模型即使声明了 `thinkingFormat`（deepseek / zai / openrouter 那些方言），
    // 探测也只用最通用、契约最明确的那个字段 —— 拿方言字段去试，
    // 试出来的"网关不认"分不清到底是"不认这一档"还是"不认这个字段"。
    let mut accepted: Option<usize> = None;
    let mut abort: Option<String> = None;
    for (index, (_level, wire)) in rungs.iter().enumerate() {
        attempts += 1;
        let response = transport
            .post(
                &endpoint,
                api,
                secret,
                trust,
                &probe_body(model_id, Some(wire), token_field),
                PROBE_TIMEOUT_SECONDS,
            )
            .await;
        if is_success(&response) {
            accepted = Some(index);
            break;
        }
        match response.status {
            // 4xx：错误文案**与思考参数无关**时再降档只会拿到同样的错 → 如实中止
            Some(code) if (400..500).contains(&code) => {
                let detail = describe(Some(&response), "");
                if !mentions_thinking(&detail) {
                    abort = Some(format!(
                        "档位 {wire} 被拒，但错误看起来与思考参数无关：{detail}"
                    ));
                    break;
                }
                // 提到 effort/reasoning/thinking → 网关不认这一档，降档再试
            }
            // 非 4xx（5xx / 429 / 连不上）：不是"不认这一档"，如实中止
            _ => {
                abort = Some(format!(
                    "档位 {wire} 请求失败：{}",
                    describe(Some(&response), "")
                ));
                break;
            }
        }
    }

    if let Some(index) = accepted {
        let wire = &rungs[index].1;
        let ladder = ladder_from(rungs, index);
        return LadderOutcome {
            accepted_top: Some(wire.clone()),
            ladder: Some(ladder),
            message: format!("网关接受的最高档位：{wire}（共 {attempts} 次请求）"),
            attempts,
        };
    }
    if let Some(message) = abort {
        return LadderOutcome::failed(message, attempts);
    }
    // ★ 一个档位都不认：**如实失败**，绝不发明一个档位、也绝不写回（纪律 5）
    LadderOutcome::failed(
        format!(
            "网关拒绝了全部 {} 个档位（多半根本不认 reasoning_effort 参数）；档位表保持原样、一个字都没写",
            rungs.len()
        ),
        attempts,
    )
}

/// 一次响应的可读摘要（状态码 + 正文；都没了就回兜底那句话）。
fn describe(response: Option<&ProbeResponse>, fallback: &str) -> String {
    let Some(response) = response else {
        return fallback.to_string();
    };
    match response.status {
        Some(code) => {
            let body = response.body.trim();
            if body.is_empty() {
                format!("HTTP {code}")
            } else {
                format!("HTTP {code}｜上游返回：{body}")
            }
        }
        None => {
            if response.detail.trim().is_empty() {
                fallback.to_string()
            } else {
                response.detail.clone()
            }
        }
    }
}

// ---------------------------------------------------------------------------
//  custom.json 的写回（纪律 4：读一遍 → 只动这一条 → 原子落盘）
// ---------------------------------------------------------------------------

/// 写回失败了的人话原因（**如实回给界面**，绝不静默）。
pub fn write_entry(
    dir: &std::path::Path,
    model_id: &str,
    efforts: &BTreeMap<String, String>,
    format: Option<&str>,
) -> Result<(), String> {
    let target = dir.join(CUSTOM_FILE);
    // 文件**不存在**是合法起点（这台机器还没手工改过目录）：从空对象起步建一份。
    // 文件**存在但读不出来**（权限 / 编码）才是"我们不该动它"——那时必须如实报。
    let raw = match std::fs::read_to_string(&target) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(format!(
                "读不到手工目录 {}（{error}）—— 实测结果没有写回，请先把它修好",
                target.display()
            ))
        }
    };
    // 空文件当"还没建"（0 字节也不是合法 JSON）；已有内容但读坏了 → 下面 upsert 会拒绝
    let raw = if raw.trim().is_empty() {
        "{}".to_string()
    } else {
        raw
    };
    let entry = OverlayEntry {
        modalities: None,
        tools: None,
        reasoning: None,
        thinking_efforts: Some(efforts.clone()),
        thinking_format: format.map(str::to_string),
    };
    let next = upsert_entry_text(&raw, model_id, &entry)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败（{}）：{e}", dir.display()))?;
    write_atomic(&target, &next)
}

/// 临时文件 + rename（**原子**：写一半断电也不会留下半份目录）。
///
/// ⚠ 临时名带上进程号：Windows 上 `fs::rename` 能覆盖，但**不保证**在另一个进程正读它时
/// 立刻成功；固定名字的 `.tmp` 还会让两个进程互相踩（本仓库踩过"偶发红"这一类坑）。
fn write_atomic(target: &std::path::Path, text: &str) -> Result<(), String> {
    let tmp = target.with_file_name(format!(
        "{}.{}.tmp",
        target
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "custom.json".into()),
        std::process::id()
    ));
    std::fs::write(&tmp, text).map_err(|e| format!("写临时文件失败（{}）：{e}", tmp.display()))?;
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("替换 {} 失败：{e}", target.display())
    })
}

/// 把一条模型的档位写进 `custom.json` 的**文本**里，返回新的整份文本。
///
/// # 为什么是"改文本"而不是"反序列化再整份重写"
///
/// `custom.json` 是**手动编写**的，其他条目（以及键顺序、缩进、写法）属于原有内容 ——
/// 整份 `serde_json` 重写会把它们全部重排/重格式化（"别的条目一个字节都不许变"这条就守不住了）。
/// 所以：条目**已经存在**时就按字节定位、只替换那一个值（见 [`splice_entry`]）；
/// 只有确实要**新增**一条（文件还没这条、或者文件都还不存在）时才整份重建
/// —— 那时本来就没有"别人的那一行"可保。
///
/// 文件是坏 JSON / 形状不对 → **报错**（绝不覆盖成一份自己造的新文件）。
pub fn upsert_entry_text(
    raw: &str,
    model_id: &str,
    entry: &OverlayEntry,
) -> Result<String, String> {
    let key = model_id.trim().to_lowercase();
    if key.is_empty() {
        return Err("模型 id 为空，无法写回".to_string());
    }
    let parsed: Value = serde_json::from_str(raw).map_err(|e| {
        format!("手工目录 custom.json 不是合法 JSON（{e}）—— 拒绝覆盖，请先手工修好")
    })?;
    let root = parsed.as_object().ok_or_else(|| {
        "手工目录 custom.json 的顶层不是 JSON 对象 —— 拒绝覆盖，请先手工修好".to_string()
    })?;
    // `entries` 缺席是**合法**的起点（`{}` / `{"source":"custom"}`）—— 补一个空表就行。
    // 仅在**结构不符**时拒绝：写入 `"entries": 3` 这类值只会破坏原有文件。
    let entries = match root.get("entries") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => {
            return Err(
                "手工目录 custom.json 的 entries 不是 JSON 对象 —— 拒绝覆盖，请先手工修好"
                    .to_string(),
            )
        }
    };

    // ★ **合并**而不是替换：原文那条里我们没碰的字段（`reasoning` / `modalities` /
    // 自行编写的其他键）必须原样保留。整条替换会把该模型的能力声明削减为仅剩档位 ——
    // 而"这条模型还支持推理吗"正是档位能不能用的前提。
    let value = overlay_entry_to_value(entry);
    if let Some(hit) = find_entry(raw, &key) {
        let merged = merge_entry_value(parse_object(&raw[hit.start..hit.end]), &value);
        return splice_entry(raw, &hit, &merged);
    }
    // 新条目也走**文本插入**（不是整份重排）：别的条目的字节同样一个都不动。
    if let Some(inserted) = insert_entry_text(raw, &key, &value) {
        return Ok(inserted);
    }

    // 兜底：文件里连 `entries` 都没有（或者位置认不出来）→ 整份重建。
    // 这条路上本来就没有"别人的那一行"可保（`{}` / `{"source":"custom"}` 这种起点）。
    let mut next = root.clone();
    let mut next_entries = entries;
    next_entries.insert(key, value);
    next.insert("entries".into(), Value::Object(next_entries));
    let mut text = serde_json::to_string_pretty(&Value::Object(next))
        .map_err(|e| format!("手工目录序列化失败：{e}"))?;
    text.push('\n');
    Ok(text)
}

/// 把新条目**插进** `entries` 对象的文本里（保住别的条目的每一个字节），返回新的整份文本。
///
/// `None` = 文件里认不出 `entries` 对象的位置（调用方整份重建）。
///
/// 缩进：先拿"已有条目的缩进"（老条目怎么排，新条目就怎么排）；一条都没有时按 `entries`
/// 那一层的缩进 + 一个两空格。`{}`（空对象）直接写成 `{ "k": {…} }`，不留奇怪的换行。
fn insert_entry_text(raw: &str, key: &str, value: &Value) -> Option<String> {
    let entries_open = locate_entries(raw)?;
    let bytes = raw.as_bytes();
    let pretty = serde_json::to_string_pretty(value).ok()?;

    let first = skip_ws(bytes, entries_open + 1);
    if bytes.get(first) == Some(&b'}') {
        // 一个条目都没有：`{ }` → `{ "k": {…} }`
        let one_line = serde_json::to_string(value).ok()?;
        let mut next = String::with_capacity(raw.len() + one_line.len() + key.len() + 6);
        next.push_str(&raw[..first]);
        next.push_str(&format!("\"{key}\": {one_line}"));
        next.push_str(&raw[first..]);
        return Some(next);
    }

    let anchor = first;
    let indent = if line_indent(raw, anchor).trim().is_empty() {
        // 老条目与 `{` 在同一行（紧凑写法）→ 沿用 entries 那层的缩进 + 两空格
        format!("{}  ", line_indent(raw, entries_open))
    } else {
        line_indent(raw, anchor)
    };
    // 新条目最后一个值的收尾位置（前面一定有逗号，要在它后面插）
    let last_end = last_entry_end(raw, entries_open)?;
    let rendered = pretty
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line.to_string()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut next = String::with_capacity(raw.len() + rendered.len() + key.len() + 8);
    next.push_str(&raw[..last_end]);
    next.push_str(&format!(",\n{indent}\"{key}\": {rendered}"));
    next.push_str(&raw[last_end..]);
    Some(next)
}

/// `entries` 那个对象 `{` 的字节位置。
fn locate_entries(raw: &str) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut cursor = skip_ws(bytes, 0);
    if bytes.get(cursor) != Some(&b'{') {
        return None;
    }
    cursor = skip_ws(bytes, cursor + 1);
    loop {
        if bytes.get(cursor) == Some(&b'}') {
            return None;
        }
        let (key_end, key_text) = string_at(raw, cursor)?;
        let colon = skip_ws(bytes, key_end);
        if bytes.get(colon) != Some(&b':') {
            return None;
        }
        let value_start = skip_ws(bytes, colon + 1);
        let (_, value_end) = skip_value(raw, value_start)?;
        if key_text.eq_ignore_ascii_case("entries") && bytes.get(value_start) == Some(&b'{') {
            return Some(value_start);
        }
        cursor = skip_ws(bytes, value_end);
        if bytes.get(cursor) == Some(&b',') {
            cursor = skip_ws(bytes, cursor + 1);
        } else {
            return None;
        }
    }
}

/// `entries` 里**最后一个条目**的值结束位置（新条目要插在它后面）。
fn last_entry_end(raw: &str, entries_open: usize) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut cursor = skip_ws(bytes, entries_open + 1);
    let mut last: Option<usize> = None;
    loop {
        if bytes.get(cursor) == Some(&b'}') {
            return last;
        }
        let (key_end, _) = string_at(raw, cursor)?;
        let colon = skip_ws(bytes, key_end);
        if bytes.get(colon) != Some(&b':') {
            return last;
        }
        let value_start = skip_ws(bytes, colon + 1);
        let (_, value_end) = skip_value(raw, value_start)?;
        last = Some(value_end);
        cursor = skip_ws(bytes, value_end);
        if bytes.get(cursor) == Some(&b',') {
            cursor = skip_ws(bytes, cursor + 1);
        } else {
            return last;
        }
    }
}

/// `OverlayEntry` → JSON（键名与 `custom.json` 里的写法一致；`None` 的维度**整个键不出现**）。
fn overlay_entry_to_value(entry: &OverlayEntry) -> Value {
    match serde_json::to_value(entry) {
        Ok(value) => value,
        // `OverlayEntry` 全是可空标量/字符串表，序列化不可能失败；真失败了也不许写坏文件
        Err(_) => Value::Null,
    }
}

/// 原文里那一段文本 → JSON 对象（解析不出来就当"没有旧值"）。
fn parse_object(text: &str) -> Option<Map<String, Value>> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .as_object()
        .cloned()
}

/// 把要写的字段**合进**已有条目：旧条目里我们没碰的键一个都不丢。
///
/// 只有本次真正要改的两个键（`thinkingEfforts` / `thinkingFormat`）是"以新值为准"，
/// 其余原样保留 —— 同时保留手动编写、未识别的键。
fn merge_entry_value(existing: Option<Map<String, Value>>, fresh: &Value) -> Value {
    let Some(fresh) = fresh.as_object() else {
        return fresh.clone();
    };
    let Some(mut merged) = existing else {
        return Value::Object(fresh.clone());
    };
    for key in ["thinkingEfforts", "thinkingFormat"] {
        match fresh.get(key) {
            Some(value) => {
                merged.insert(key.to_string(), value.clone());
            }
            // 新值里没有这一项（比如模型没有方言）→ 旧的那一项要**删掉**：
            // 留着就成了"我们没试过的档位/方言"，而这次校准的意义就是不留没试过的东西
            None => {
                merged.shift_remove(key);
            }
        }
    }
    Value::Object(merged)
}

/// `custom.json` 里那条模型条目在**原文里的字节范围**。
#[derive(Debug, Clone, PartialEq, Eq)]
struct EntrySpan {
    /// 值（那个对象）的起止字节
    start: usize,
    end: usize,
}

/// 在原文里定位 `entries` 下指定 key 的**值**的字节范围。
///
/// 手写一个只认结构的扫描器（不解析值）：字符串要处理转义，嵌套对象/数组要配平 ——
/// 十几行换来"别的条目一个字节都不动"，值。
///
/// ⚠ key 比对**大小写不敏感**（目录加载时统一小写，见 `capability_catalog::load_layer`），
/// 但**替换的是原文里那个值的字节**，所以文件里写的是 `GPT-4` 也照样命中。
///
/// ⚠ 只认**最外层**那个 `entries`（值里嵌一个同名键不算）—— 与 `capability_catalog`
/// 的读法一致；识别错误会修改其他数据。
fn find_entry(raw: &str, key: &str) -> Option<EntrySpan> {
    let bytes = raw.as_bytes();
    let mut cursor = skip_ws(bytes, 0);
    if bytes.get(cursor) != Some(&b'{') {
        return None;
    }
    cursor = skip_ws(bytes, cursor + 1);
    loop {
        if bytes.get(cursor) == Some(&b'}') {
            return None;
        }
        // 当前位置必须是一个键（非字符串键的 JSON 在解析那一关就已经被拒了）
        let (key_end, key_text) = string_at(raw, cursor)?;
        let is_entries = key_text.eq_ignore_ascii_case("entries");
        let colon = skip_ws(bytes, key_end);
        if bytes.get(colon) != Some(&b':') {
            return None;
        }
        let value_start = skip_ws(bytes, colon + 1);
        let (_, value_end) = skip_value(raw, value_start)?;
        if is_entries && bytes.get(value_start) == Some(&b'{') {
            return find_in_entries(raw, value_start, key);
        }
        cursor = skip_ws(bytes, value_end);
        if bytes.get(cursor) == Some(&b',') {
            cursor = skip_ws(bytes, cursor + 1);
        } else {
            return None;
        }
    }
}

/// 在 `entries` 那个对象里找 `key` 的值。
fn find_in_entries(raw: &str, entries_open: usize, key: &str) -> Option<EntrySpan> {
    walk_entries(raw, entries_open, key)
}

/// 在 `entries` 对象里逐个条目走一遍；命中就回它的**值**的范围，没命中回 `None`。
fn walk_entries(raw: &str, entries_open: usize, key: &str) -> Option<EntrySpan> {
    let bytes = raw.as_bytes();
    let mut cursor = skip_ws(bytes, entries_open + 1);
    loop {
        if bytes.get(cursor) == Some(&b'}') {
            return None;
        }
        let (key_end, key_text) = string_at(raw, cursor)?;
        let colon = skip_ws(bytes, key_end);
        if bytes.get(colon) != Some(&b':') {
            return None;
        }
        let value_start = skip_ws(bytes, colon + 1);
        let (_, value_end) = skip_value(raw, value_start)?;
        if key_text.eq_ignore_ascii_case(key) {
            return Some(EntrySpan {
                start: value_start,
                end: value_end,
            });
        }
        cursor = skip_ws(bytes, value_end);
        if bytes.get(cursor) == Some(&b',') {
            cursor = skip_ws(bytes, cursor + 1);
        } else {
            return None;
        }
    }
}

/// 读一个 JSON 字符串：返回 `(结束位置, 字符串内容)`。**内容不带引号**。
///
/// ⚠ 位置运算必须一次算清（结束位置就是"下一个待处理字节"）—— 先按 `cursor..key_end`
/// 取出带引号的原文再裁，中间的偏移量极容易算错，而算错的后果是**切片 panic** 或改错字节。
fn string_at(raw: &str, start: usize) -> Option<(usize, &str)> {
    let (_, end) = skip_string(raw, start)?;
    Some((end, &raw[start + 1..end - 1]))
}

/// 跳过一个 JSON 字符串（含转义），返回 `(起, 止)` 的字节位置。
fn skip_string(raw: &str, start: usize) -> Option<(usize, usize)> {
    let bytes = raw.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Some((start, index + 1)),
            _ => index += 1,
        }
    }
    None
}

/// 跳过一个 JSON 值（对象/数组按括号配平，字符串走 [`skip_string`]，标量走到分隔符为止）。
fn skip_value(raw: &str, start: usize) -> Option<(usize, usize)> {
    let bytes = raw.as_bytes();
    let first = *bytes.get(start)?;
    if first == b'"' {
        return skip_string(raw, start);
    }
    if first != b'{' && first != b'[' {
        let mut index = start;
        while index < bytes.len()
            && !matches!(
                bytes[index],
                b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t'
            )
        {
            index += 1;
        }
        return Some((start, index));
    }
    let mut depth = 0usize;
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index = skip_string(raw, index)?.1;
                continue;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((start, index + 1));
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// 从 `start` 起跳过空白，返回下一个非空白字节的位置。
fn skip_ws(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

/// 把新值按**目标那一段的缩进**重新排版，然后只替换那一段字节。
///
/// 目标值那一行的第一个非空白字节前面就是它的缩进（原文怎么排的，新值就跟着怎么排）——
/// 同一条目内部不追求"字节不变"（那份数据已经过期了），但**别的条目与整份文件的排版一个都不动**。
fn splice_entry(raw: &str, span: &EntrySpan, value: &Value) -> Result<String, String> {
    // 内联值也要能替换（原文件里 `"m": {"off":"none"}` 是合法的）
    if !raw[span.start..span.end].contains('\n') {
        let one_line =
            serde_json::to_string(value).map_err(|e| format!("序列化新条目失败：{e}"))?;
        let mut next = String::with_capacity(raw.len() + one_line.len());
        next.push_str(&raw[..span.start]);
        next.push_str(&one_line);
        next.push_str(&raw[span.end..]);
        return Ok(next);
    }
    let indent = line_indent(raw, span.start);
    let pretty =
        serde_json::to_string_pretty(value).map_err(|e| format!("序列化新条目失败：{e}"))?;
    let rendered = pretty
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line.to_string()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut next = String::with_capacity(raw.len() + rendered.len());
    next.push_str(&raw[..span.start]);
    next.push_str(&rendered);
    next.push_str(&raw[span.end..]);
    Ok(next)
}

/// 那一行从行首到 `position` 的空白（新值的每一行都要有同样的缩进）。
fn line_indent(raw: &str, position: usize) -> String {
    let head = &raw[..position.min(raw.len())];
    let line_start = head.rfind('\n').map(|index| index + 1).unwrap_or(0);
    head[line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

// ---------------------------------------------------------------------------
//  整个端点（一次校准一批）
// ---------------------------------------------------------------------------

/// `POST /api/ai/capabilities/probe` 的请求体（Kotlin `CapabilityProbeRequest`）。
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeRequest {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
}

/// 一条 outcome（键名与 Dart 的 `AiCapabilityProbeOutcome` 逐字一致）。
///
/// `acceptedTop` / `ladder` **没有就不出现**（Kotlin `explicitNulls = false`；
/// Dart 读的是 `json['acceptedTop'] as String?`，缺键与 null 等价）——
/// 写成 `"acceptedTop": null` 会让"没试成"与"没试过"看起来一样。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeOutcome {
    pub provider_id: String,
    pub model_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_top: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ladder: Option<BTreeMap<String, String>>,
    pub written: bool,
    pub skipped: bool,
    pub message: String,
}

impl ProbeOutcome {
    fn skipped(provider_id: &str, model_id: &str, message: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            ok: false,
            accepted_top: None,
            ladder: None,
            written: false,
            skipped: true,
            message: message.into(),
        }
    }

    fn failed(provider_id: &str, model_id: &str, message: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            ok: false,
            accepted_top: None,
            ladder: None,
            written: false,
            skipped: false,
            message: message.into(),
        }
    }

    fn json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// 整个端点的响应（`{ok, message, outcomes:[…]}`）。
pub fn response_json(ok: bool, message: impl Into<String>, outcomes: &[ProbeOutcome]) -> Value {
    serde_json::json!({
        "ok": ok,
        "message": message.into(),
        "outcomes": outcomes.iter().map(ProbeOutcome::json).collect::<Vec<_>>(),
    })
}

/// 开关没开时的那句话（**必须说清"它会花钱"**）。
pub fn disabled_message() -> &'static str {
    "实测校准没开（它会对每个模型发真实请求、产生费用）—— 设置页先打开「允许实测校准」"
}

/// 一次校准的入口。`dir` = `<storage>\ai\capabilities`。
///
/// 顺序是刻意的：
///  1. **先判开关**（关着一个请求都不发，也**不读库**）；
///  2. **先读一遍 `custom.json`**（坏了就当场如实报，别等到探测花钱花完了才发现写不回去）；
///  3. 再列 Provider / 模型、逐模型独立探测。
pub async fn probe<T: Transport>(
    state: &AppState,
    dir: &std::path::Path,
    request: &ProbeRequest,
    transport: &T,
) -> Result<Value, ApiError> {
    // ① 开关（纪律 1）
    if !crate::ai::ai_settings::probe_enabled(state)? {
        return Ok(response_json(false, disabled_message(), &[]));
    }

    // ② 手工目录先读一遍：坏 JSON 当场如实报（绝不"覆盖成一份新的空白文件"）
    let target = dir.join(CUSTOM_FILE);
    if let Ok(raw) = std::fs::read_to_string(&target) {
        if let Err(message) = upsert_entry_text(&raw, "预检", &empty_entry()) {
            return Ok(response_json(
                false,
                format!("{message}（实测校准没有开始：写回目标不可用，先修好它再试）"),
                &[],
            ));
        }
    }

    let providers = match request
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(id) => vec![crate::ai::ai::require_provider_row(state, id)?],
        // 缺省 = 所有**已启用**的 Provider。逐条回查一次是为了拿到行（行里才有 base_url /
        // 凭据引用）；查不到的那种"并发里刚好被删了"按**跳过**处理，不把整批拖死。
        None => crate::ai::ai::list_providers(state)?
            .into_iter()
            .filter(|p| p.enabled)
            .filter_map(|dto| {
                crate::ai::ai::get_provider_row(state, &dto.id)
                    .ok()
                    .flatten()
            })
            .collect(),
    };
    if providers.is_empty() {
        return Ok(response_json(
            false,
            "没有可校准的 Provider（缺省只校准**已启用**的那些）",
            &[],
        ));
    }

    let mut outcomes: Vec<ProbeOutcome> = Vec::new();
    for provider in &providers {
        probe_provider(
            state,
            dir,
            provider,
            request.model_id.as_deref(),
            &mut outcomes,
            transport,
        )
        .await?;
    }

    let ok_count = outcomes.iter().filter(|o| o.ok).count();
    let fail_count = outcomes.iter().filter(|o| !o.ok && !o.skipped).count();
    let skip_count = outcomes.iter().filter(|o| o.skipped).count();
    Ok(response_json(
        fail_count == 0 && !outcomes.is_empty(),
        format!(
            "校准完成：成功 {ok_count}、失败 {fail_count}、跳过 {skip_count}（每个 Provider 最多 {MAX_MODELS_PER_PROVIDER} 个）"
        ),
        &outcomes,
    ))
}

/// 一个 Provider 的全部目标模型（**逐模型独立**：一个失败接着试下一个）。
async fn probe_provider<T: Transport>(
    state: &AppState,
    dir: &std::path::Path,
    provider: &AiProviderRow,
    model_id: Option<&str>,
    outcomes: &mut Vec<ProbeOutcome>,
    transport: &T,
) -> Result<(), ApiError> {
    let secret = crate::provider::provider_probe::resolve_secret(state, provider);
    let Some(api) = crate::provider::protocol::Api::parse(&provider.api) else {
        outcomes.push(ProbeOutcome::skipped(
            &provider.id,
            "",
            format!("未知协议：{}，跳过整个 Provider", provider.api),
        ));
        return Ok(());
    };
    // 只支持 OpenAI 兼容 chat 端点（Kotlin 同：另外两家没有 `reasoning_effort` 这个线上表达）
    if api != crate::provider::protocol::Api::OpenAiCompletions {
        outcomes.push(ProbeOutcome::skipped(
            &provider.id,
            "",
            format!(
                "思考校准只支持 OpenAI 兼容 chat 端点（该 Provider 是 {}）",
                provider.api
            ),
        ));
        return Ok(());
    }
    if provider
        .credential_ref
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .is_some()
        && secret.is_none()
    {
        outcomes.push(ProbeOutcome::skipped(
            &provider.id,
            "",
            format!(
                "凭据 {} 未配置，跳过整个 Provider",
                provider.credential_ref.clone().unwrap_or_default()
            ),
        ));
        return Ok(());
    }

    // 判据沿用既有目录查找：`enabled && reasoning`（缺省 = 全部推理模型）
    let models: Vec<crate::ai::ai::AiModelDto> = crate::ai::ai::list_models(state, &provider.id)?
        .into_iter()
        .filter(|m| m.enabled && m.reasoning)
        .filter(
            |m| match model_id.map(str::trim).filter(|s| !s.is_empty()) {
                Some(want) => m.id == want,
                None => true,
            },
        )
        .take(MAX_MODELS_PER_PROVIDER)
        .collect();

    let trust = crate::web::endpoint_guard::parse_trust(&provider.endpoint_trust)
        .unwrap_or_else(|| crate::web::endpoint_guard::infer_trust(""));

    for model in models {
        outcomes.push(probe_one(provider, &model, secret.as_deref(), trust, dir, transport).await);
    }
    Ok(())
}

/// 一个模型：探测 → （成功时）写回 `custom.json`。**任何一步失败都只影响这一条**。
async fn probe_one<T: Transport>(
    provider: &AiProviderRow,
    model: &crate::ai::ai::AiModelDto,
    secret: Option<&str>,
    trust: &str,
    dir: &std::path::Path,
    transport: &T,
) -> ProbeOutcome {
    let rungs = rungs_for(&model.thinking_efforts);
    let outcome = run_ladder(
        transport,
        &provider.base_url,
        provider.api.as_str(),
        &model.id,
        secret,
        trust,
        &rungs,
    )
    .await;

    let Some(ladder) = outcome.ladder.clone() else {
        // 纪律 5：不猜、不写。`acceptedTop` 整个键都不出现（不是 null）
        return ProbeOutcome::failed(
            &provider.id,
            &model.id,
            format!("{}（{} 次请求）", outcome.message, outcome.attempts),
        );
    };

    // 写回：只动这一个模型的档位（+ 必要时方言）
    match write_entry(dir, &model.id, &ladder, model.thinking_format.as_deref()) {
        Ok(()) => ProbeOutcome {
            provider_id: provider.id.clone(),
            model_id: model.id.clone(),
            ok: true,
            accepted_top: outcome.accepted_top.clone(),
            ladder: Some(ladder),
            written: true,
            skipped: false,
            message: format!(
                "{}｜已写回 custom.json（{} 次请求）",
                outcome.message, outcome.attempts
            ),
        },
        // 探测成功但无法写回：**ok=false**（目标是"档位写回"，并需说明未写成功的原因）
        Err(message) => ProbeOutcome::failed(
            &provider.id,
            &model.id,
            format!("{}｜但写回失败：{message}", outcome.message),
        ),
    }
}

/// 预检用的空条目（只用来验证 `custom.json` 能不能被改，写完就丢）。
///
/// ⚠ 预检**不写盘**：它只走一遍"读 → 定位/新建 → 序列化"，就是为了在花掉第一个请求之前
/// 发现"这份文件根本改不了"（坏 JSON / 形状不对）。
fn empty_entry() -> OverlayEntry {
    OverlayEntry {
        modalities: None,
        tools: None,
        reasoning: None,
        thinking_efforts: None,
        thinking_format: None,
    }
}

// ---------------------------------------------------------------------------
//  测试（**全部离线**：注入假 transport，一条网都不出）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
