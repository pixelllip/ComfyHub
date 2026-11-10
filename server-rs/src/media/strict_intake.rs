//! 附件准入（AIH-028 / AIH-029 / AIH-030）—— Kotlin `StrictIntake` + `AttachmentPolicy` +
//! `InlineBudget` + `AdapterCapabilities.transportsFor` 的逐条移植。
//!
//! # 这一层在防什么
//!
//! R9 的**三层准入**：① 选完预检（前端调 `POST /api/ai/preflight`）→ ② 发送前前端禁用按钮 →
//! ③ **创建 Run 之前后端用库里的事实再验一次**。本模块就是①②③共用的那份规则。
//!
//! 两条不变式（`e2e-ai-tools-test.ps1` 第 9 幕盯的就是它们）：
//!
//!  1. 不通过 → **`400 UNSUPPORTED_CONTENT`**，且**上游请求数为 0**（在 append 请求消息之前就返回）；
//!  2. **不许把 blocker 降级成占位符再继续发**（那正是 AIH-030 要防的"看起来发出去了、其实没有"）。
//!
//! # 判定权在后端手里
//!
//! 前端声明的模态只是**线索**：只要拿到文件头，就以 [`crate::media::file_kind::detect`] 的判定为准 ——
//! "前端谎报 image 骗过准入"这条路径不通（AIH-027）。
//!
//! # 传输方式的两个来源（这条最容易写错）
//!
//! 传输方式是**协议适配器**的属性（"这条路真的实现了吗"），而模型声明的是"能不能吃这种输入"。
//! 模型**显式声明了**就用它与适配器的**交集**；**没声明**就用适配器实现的那种 ——
//! 否则内置目录里那些只声明模态、不声明传输的模型**永远发不出图片**
//! （AGENTS §10 记过这条：不回落的后果就是"图片永远发不出去"）。

use crate::ai::ai::AiModelDto;
use crate::media::file_kind::{self, FileKind};

/// 允许前端提交的文件头大小上限；超过就截断，避免拿它当上传通道。
pub const MAX_HEAD_BYTES: usize = 4096;

/// 内联 base64 的原始字节上限（与 [`crate::media::attachments::MAX_INLINE_BYTES`] 同一个口径）。
pub const MAX_INLINE_BYTES: i64 = 8 * 1024 * 1024;

/// 单次请求内联附件的原始字节总预算（base64 后约 ×1.33）。
pub const MAX_TOTAL_INLINE_BYTES: i64 = 20 * 1024 * 1024;

/// 附件的客观事实（由严格分类器产出，AIH-027）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentFact {
    pub name: String,
    /// image / video / audio / document / text；`None` = 认不出来
    pub modality: Option<String>,
    pub mime_type: String,
    pub size_bytes: i64,
    /// 图片像素预算；非图片为 `None`
    pub pixels: Option<i64>,
}

/// 准入结论。
///
/// `allowed == false` 时**必须**阻断发送；调用方不允许把 blocker 降级成占位符后继续请求上游。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightResult {
    pub allowed: bool,
    pub blockers: Vec<String>,
}

impl PreflightResult {
    pub fn allow() -> Self {
        Self {
            allowed: true,
            blockers: Vec::new(),
        }
    }

    pub fn block(reasons: Vec<String>) -> Self {
        Self {
            allowed: false,
            blockers: reasons,
        }
    }
}

/// 一次"严格判定"的结果：客观事实 + 阻断理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub fact: AttachmentFact,
    pub blockers: Vec<String>,
}

/// 入库 / 预检前的严格判定（AIH-027）。
///
/// `head` 为 `None` / 空时只能采信声明（Kotlin 同：不把"未经签名确认"说成确认过的）。
pub fn resolve(
    name: &str,
    declared_modality: Option<&str>,
    declared_mime: &str,
    size_bytes: i64,
    pixels: Option<i64>,
    head: Option<&[u8]>,
) -> Resolved {
    let declared = declared_modality
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| m.to_string());
    let Some(head) = head.filter(|h| !h.is_empty()) else {
        return Resolved {
            fact: AttachmentFact {
                name: name.to_string(),
                modality: declared,
                mime_type: declared_mime.to_string(),
                size_bytes,
                pixels,
            },
            blockers: Vec::new(),
        };
    };
    let trimmed: Vec<u8> = head.iter().take(MAX_HEAD_BYTES).copied().collect();
    let detected = file_kind::detect(&trimmed, name, Some(declared_mime));
    if detected.kind == FileKind::Unknown {
        return Resolved {
            fact: AttachmentFact {
                name: name.to_string(),
                modality: None,
                mime_type: detected
                    .mime_type
                    .unwrap_or_else(|| declared_mime.to_string()),
                size_bytes,
                pixels,
            },
            blockers: vec![format!(
                "无法识别文件 {name} 的真实类型（签名不匹配声明的 {declared_mime}），已阻断"
            )],
        };
    }
    Resolved {
        fact: AttachmentFact {
            name: name.to_string(),
            modality: detected.kind.modality().map(|m| m.to_string()),
            mime_type: detected
                .mime_type
                .unwrap_or_else(|| declared_mime.to_string()),
            size_bytes,
            pixels,
        },
        blockers: Vec::new(),
    }
}

/// 某个协议适配器在**这个模态**上真的实现了哪些传输方式。
///
/// 与 Kotlin `AdapterCapabilities.transportsFor` 同一张表：**三家都只实现了图片的
/// `inline_base64`**（视频 / 音频 / 文档 / 文本一律空集 → 预检会如实说"尚未实现"）。
pub fn adapter_transports(api: &str, modality: &str) -> Vec<&'static str> {
    let known_api = matches!(
        api,
        "openai-completions" | "openai-responses" | "anthropic-messages"
    );
    if known_api && modality == "image" {
        vec!["inline_base64"]
    } else {
        Vec::new()
    }
}

fn modality_label(modality: &str) -> &'static str {
    match modality {
        "image" => "图片",
        "video" => "视频",
        "audio" => "音频",
        "document" => "文档",
        _ => "文本",
    }
}

fn matches_mime(allowlist: &[String], mime: &str) -> bool {
    let mime = mime.to_lowercase();
    allowlist.iter().any(|rule| {
        let rule = rule.trim().to_lowercase();
        if rule.is_empty() {
            return false;
        }
        if rule == mime {
            return true;
        }
        match rule.strip_suffix('*') {
            Some(prefix) => mime.starts_with(prefix),
            None => false,
        }
    })
}

/// 模型声明 + 适配器实现 + MIME 白名单 + 大小/数量/像素预算，全部满足才放行。
///
/// `adapter_transports` 是该模态在这个协议适配器上**真的实现了**的传输方式。
pub fn evaluate(
    model: &AiModelDto,
    attachment: &AttachmentFact,
    adapter_transports: &[&str],
    current_attachment_count: i64,
) -> PreflightResult {
    let mut blockers: Vec<String> = Vec::new();
    let Some(modality) = attachment.modality.clone() else {
        // 文本附件走正文抽取，不算多模态输入；这里只做类型未知的阻断
        blockers.push(format!(
            "无法识别文件类型（{}），已阻断；请确认为受支持的图片或文档",
            attachment.mime_type
        ));
        return PreflightResult::block(blockers);
    };
    if modality == "text" {
        return PreflightResult::allow();
    }

    if !model.input_modalities.iter().any(|m| m == &modality) {
        blockers.push(format!(
            "模型 {} 未声明支持{}输入",
            model.display_name,
            modality_label(&modality)
        ));
    }

    // 传输方式是**协议**的属性，模型声明的是"能不能吃这种输入"。
    let declared: Vec<String> = model
        .attachment_transports
        .get(&modality)
        .cloned()
        .unwrap_or_default();
    let usable: Vec<String> = if declared.is_empty() {
        adapter_transports.iter().map(|t| t.to_string()).collect()
    } else {
        declared
            .iter()
            .filter(|t| adapter_transports.iter().any(|a| a == &t.as_str()))
            .cloned()
            .collect()
    };
    if usable.is_empty() {
        blockers.push(if declared.is_empty() {
            format!(
                "{}的传输方式当前协议适配器尚未实现",
                modality_label(&modality)
            )
        } else {
            format!(
                "{}的传输方式（{}）当前协议适配器尚未实现",
                modality_label(&modality),
                declared.join("、")
            )
        });
    } else if usable.iter().any(|t| t == "inline_base64")
        && attachment.size_bytes > MAX_INLINE_BYTES
    {
        blockers.push(format!(
            "文件 {} 超过内联发送上限（{} MB），请先压缩或换一张更小的图",
            attachment.name,
            MAX_INLINE_BYTES / 1024 / 1024
        ));
    }

    if let Some(max_bytes) = model.max_attachment_bytes {
        if attachment.size_bytes > max_bytes {
            blockers.push(format!(
                "文件 {} 超过大小上限（{} MB）",
                attachment.name,
                max_bytes / 1024 / 1024
            ));
        }
    }
    if let Some(max_count) = model.max_attachment_count {
        if current_attachment_count >= max_count {
            blockers.push(format!("附件数量超过上限（{max_count}）"));
        }
    }
    if !model.mime_allowlist.is_empty()
        && !matches_mime(&model.mime_allowlist, &attachment.mime_type)
    {
        blockers.push(format!("该模型不接受 {} 类型", attachment.mime_type));
    }

    if blockers.is_empty() {
        PreflightResult::allow()
    } else {
        PreflightResult::block(blockers)
    }
}

/// 一次请求里"哪些附件真的发出去"的判定（**纯函数便于单测**）。
///
/// 一次对话可能累积好几张历史图片，而请求体不能无限大：超过预算的那些**不发**，
/// 但调用方必须把它们变成一条**明确的说明**（"这张图没随本次请求发送"），
/// 而不是静默消失 —— AIH-030 要防的就是"看起来发出去了、其实没有"。
///
/// 这是"贪心 + 保序"：从最新到最旧由调用方决定顺序，装不下就跳过、继续试后面的小图。
pub fn inline_budget_plan(sizes: &[i64]) -> Vec<bool> {
    inline_budget_plan_with(sizes, MAX_TOTAL_INLINE_BYTES, MAX_INLINE_BYTES)
}

pub fn inline_budget_plan_with(sizes: &[i64], max_total: i64, max_single: i64) -> Vec<bool> {
    let mut total = 0i64;
    sizes
        .iter()
        .map(|size| {
            let ok = *size >= 1 && *size <= max_single && total + *size <= max_total;
            if ok {
                total += *size;
            }
            ok
        })
        .collect()
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
