//! `comfy_view_outputs`：让模型**真的看见**用户（或它自己）在 ComfyUI 里跑出来的产物。
//!
//! 现场（2026-10-29 用户）："我能不能让 AI 看我直接在 comfy 运行的产物（这样就不用
//! 在对话中重复发送图片了）？" —— 自动捕获早就把这些图收进画廊了，但模型手里只有
//! `mediaId` / 文件名，没有像素，于是只能照着提示词把画面"编"出来。
//!
//! 做法只有一步：读 `media_assets` 里最近的图片 → 返回 `referenceImages`（**与
//! `comfy_load_workflow` 同一个字段**）。执行器 [`crate::ai::reference_attach`] 会把它们
//! 缩到 ≤1024 长边、挂到工具结果之后的一条 `user` 轮上 —— 能力判据（模型声明图片模态 +
//! 协议支持 `inline_base64`）、每 Run 预算、按路径去重**全部复用那一份**，这里不另写一套。
//!
//! # 三条边界
//!
//! 1. **只读**：不写库、不复制文件、不碰 ComfyUI（所以出厂 `ALLOW`，也不吃 ComfyUI 查询额度）。
//! 2. **只附图片**：视频 / 音频没有 `inline_base64` 那条传输（附件协议同理），照实列出来
//!    但写明"这条看不了" —— 不许假装看到了。
//! 3. **自动捕获有延迟**：刚在 ComfyUI 里跑完的那一两张可能还没轮询入库。空结果要如实说
//!    "等几秒再试 / 确认自动捕获开着"，**绝不许**凭文件名编画面。

use rusqlite::params;
use serde_json::{json, Value};

use crate::tools::tools::{ToolFailure, ToolOutcome};
use crate::web::routes::AppState;

/// 一次最多列几张（与 [`crate::ai::reference_attach::MAX_PER_RESULT`] 对齐：
/// 列更多也没用，附不进去的会被如实写进"没附上的"那句）。
const MAX_LIMIT: i64 = 8;
/// 默认列几张。
const DEFAULT_LIMIT: i64 = 3;
/// 正文里提示词片段的截断长度（完整正文在库里，模型要写东西时用 `search_prompts`）。
const PROMPT_CLIP: usize = 240;

struct MediaRow {
    id: i64,
    kind: String,
    original_name: String,
    stored_name: String,
    width: Option<i64>,
    height: Option<i64>,
    created_at: Option<String>,
    source: Option<String>,
    prompt_title: Option<String>,
    positive_prompt: Option<String>,
}

/// `comfy_view_outputs` 的实现（`limit` / `mediaId` 二选一，都不给就取最近的）。
pub fn tool_view_outputs(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let limit = arguments
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    let media_id = arguments.get("mediaId").and_then(Value::as_i64);

    let rows = load(state, media_id, limit)
        .map_err(|error| ToolFailure::new("INTERNAL", format!("读取画廊产物失败：{error:?}")))?;

    if rows.is_empty() {
        let text = match media_id {
            Some(id) => format!(
                "画廊里没有 id={id} 的产物（可能已被删除，或 mediaId 写错了）。\
                 用不带 mediaId 的调用看最近产物；刚跑完的图通常要等几秒才被自动捕获入库。"
            ),
            None => "画廊里还没有图片产物。\n\
                     可能的原因：① 刚在 ComfyUI 里跑完，自动捕获还没轮询到（通常几秒）；\
                     ② 设置页里的「ComfyUI 自动捕获」没开；\
                     ③ 产物还在 ComfyUI 的 output 目录、没入库。\n\
                     这时**不要**猜画面内容 —— 可以请用户稍等再试，或直接从画廊拖一张图进对话。"
                .to_string(),
        };
        return Ok(ToolOutcome::new(
            text,
            json!({ "count": 0, "items": [], "referenceImages": [] }),
        ));
    }

    let mut text = String::new();
    if media_id.is_some() {
        text.push_str("画廊里的这一条产物（只读）：\n");
    } else {
        text.push_str(&format!("画廊里最近的 {} 条产物（只读）：\n", rows.len()));
    }
    let mut items: Vec<Value> = Vec::new();
    let mut references: Vec<Value> = Vec::new();

    for row in &rows {
        let path = crate::media::media::resolve_media(state.media_dir(), &row.stored_name);
        let is_image = row.kind.eq_ignore_ascii_case("IMAGE");
        let size = match (row.width, row.height) {
            (Some(width), Some(height)) => format!("{width}×{height}"),
            _ => "尺寸未知".to_string(),
        };
        text.push_str(&format!(
            "\n── #{}{} · {} · {}{}{}\n",
            row.id,
            row.prompt_title
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .map(|t| format!("「{}」", clip(t, 60)))
                .unwrap_or_default(),
            row.kind,
            size,
            row.source
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| format!(" · 来源 {s}"))
                .unwrap_or_default(),
            row.created_at
                .as_deref()
                .filter(|c| !c.trim().is_empty())
                .map(|c| format!(" · {c}"))
                .unwrap_or_default(),
        ));
        text.push_str(&format!("   文件：{}\n", row.original_name));
        if let Some(prompt) = row
            .positive_prompt
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            text.push_str(&format!("   正面提示词：{}\n", clip(prompt, PROMPT_CLIP)));
        }

        if is_image {
            match path.as_deref() {
                Some(path) => {
                    let path_text = path.to_string_lossy().to_string();
                    // 与 `comfy_load_workflow` 的 `referenceImages` 同一个形状；
                    // `label` 让执行器写"产物 #N"而不是"节点 N"。
                    references.push(json!({
                        "nodeId": row.id.to_string(),
                        "label": format!("产物 #{}", row.id),
                        "file": row.original_name,
                        "path": path_text,
                        "exists": true,
                        "width": row.width,
                        "height": row.height,
                    }));
                    text.push_str("   路径：");
                    text.push_str(&path_text);
                    text.push('\n');
                }
                None => {
                    text.push_str("   （文件已不在 storage 里，没法附给模型）\n");
                }
            }
        } else {
            text.push_str("   （视频 / 音频看不了画面，这里只有元数据）\n");
        }

        items.push(json!({
            "id": row.id,
            "kind": row.kind,
            "originalName": row.original_name,
            "width": row.width,
            "height": row.height,
            "source": row.source,
            "createdAt": row.created_at,
            "promptTitle": row.prompt_title,
            "path": path.as_ref().map(|p| p.to_string_lossy().to_string()),
        }));
    }

    let attached = references.len();
    text.push('\n');
    if attached > 0 {
        text.push_str(&format!(
            "（能研究图片的模型会自动收到其中 {attached} 张的缩图 —— 原图尺寸见上；\
             看不到图时按路径请用户确认，**不要**凭文件名编画面内容。）"
        ));
    } else {
        text.push_str(
            "（这些产物里没有可以直接附上的图片；要我看画面，请把图拖进对话，\
             或者等自动捕获把图片入库后再调用一次。）",
        );
    }

    let payload = json!({
        "count": rows.len(),
        "items": items,
        "referenceImages": references,
    });
    Ok(ToolOutcome::new(text, payload))
}

fn load(
    state: &AppState,
    media_id: Option<i64>,
    limit: i64,
) -> Result<Vec<MediaRow>, crate::web::routes::ApiError> {
    state.with(|conn| {
        let sql = if media_id.is_some() {
            format!("{SELECT} WHERE m.id = ?1")
        } else {
            format!(
                "{SELECT} WHERE m.kind = 'IMAGE' ORDER BY m.created_at DESC, m.id DESC LIMIT ?1"
            )
        };
        let mut stmt = conn.prepare(&sql)?;
        let rows = if let Some(id) = media_id {
            stmt.query_map(params![id], row_of)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map(params![limit], row_of)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(rows)
    })
}

const SELECT: &str = "SELECT m.id, m.kind, m.original_name, m.stored_name, m.width, m.height,
        m.created_at, m.source,
        p.title AS prompt_title, p.positive_prompt AS positive_prompt
   FROM media_assets m LEFT JOIN prompts p ON p.id = m.prompt_id";

fn row_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaRow> {
    Ok(MediaRow {
        id: row.get("id")?,
        kind: row
            .get::<_, Option<String>>("kind")?
            .unwrap_or_else(|| "IMAGE".to_string()),
        original_name: row.get("original_name")?,
        stored_name: row.get("stored_name")?,
        width: row.get("width")?,
        height: row.get("height")?,
        created_at: crate::infra::iso_time::iso_time(
            row.get::<_, Option<String>>("created_at")?.as_deref(),
        ),
        source: row.get("source")?,
        prompt_title: row.get("prompt_title")?,
        positive_prompt: row.get("positive_prompt")?,
    })
}

/// 按字符（不是字节）截断，末尾补省略号。
fn clip(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}
