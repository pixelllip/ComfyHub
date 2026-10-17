//! 产物写侧的另一半（P4 第 4 步）：`PATCH` / `DELETE` + `/api/stats`。
//!
//! 这一批**刻意不碰**需要新依赖的三件事（下一批再说）：
//!  - `POST /api/media/upload`：multipart 解析（axum 要开 `multipart` feature）；
//!  - `GET /api/media/{id}/thumb`：图像解码 + 缩放（要 `image` crate，替掉 Kotlin 的 ImageIO + TwelveMonkeys）；
//!  - `GET /api/media/{id}/poster`：ffmpeg 抽帧。
//!
//! # `PATCH` 的两条语义（与 Kotlin `MediaRepo.update` 逐条对齐）
//!
//! 1. **`clearPrompt: true` 与 `promptId: null` 不是一回事**：前者是"明确解除关联"
//!    （把 `prompt_id` 写成 NULL），后者是"这次不动这一列"。少了 `clearPrompt`，
//!    界面上就永远解绑不了一条产物。
//! 2. **一个字段都没有 → 不是 200 空转，而是 404**（Kotlin 那边返回 `false`，
//!    路由翻成"产物 $id 不存在或无字段需要更新"）。这条看着怪，但它让"前端发了个空 body"
//!    这种 bug 立刻可见，而不是静默成功。
//!
//! # `DELETE` 要连磁盘一起清
//!
//! 只删库不删文件 = 磁盘上的孤儿会一直涨（画廊里看不到、也永远不会被清理）。
//! 所以这里按 Kotlin `storage.deleteMediaFile` + `deleteThumb` 的顺序：
//! 先删库（事务里），成功后再删文件（**删除失败只当警告**：库已经删了，
//! 为一次 unlink 失败回 500 会让界面以为"没删掉"，反而更糟）。

use rusqlite::{params, OptionalExtension};

use crate::web::inputs::MediaUpdate;
use crate::media::media;
use crate::web::models::MediaDto;
use crate::library::repo::{self, WriteError};
use crate::web::routes::{ApiError, AppState};

/// 改标题 / 关联 / 收藏 / 备注 / NSFW 标记。
pub fn update(state: &AppState, id: i64, input: &MediaUpdate) -> Result<MediaDto, ApiError> {
    // 关联的提示词必须存在（Kotlin 同样先查一次再改）
    if let Some(prompt_id) = input.prompt_id.filter(|_| !input.clear_prompt) {
        let exists = state.with(|conn| {
            Ok(conn
                .query_row("SELECT id FROM prompts WHERE id = ?1", params![prompt_id], |row| {
                    row.get::<_, i64>(0)
                })
                .optional()?
                .is_some())
        })?;
        if !exists {
            return Err(ApiError::bad_request(format!("promptId={prompt_id} 对应的提示词不存在")));
        }
    }

    let ok = state.write(|tx| -> Result<bool, WriteError> {
        let mut sets: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if input.clear_prompt {
            sets.push("prompt_id = ?".into());
            args.push(Box::new(rusqlite::types::Null));
        } else if let Some(prompt_id) = input.prompt_id {
            sets.push("prompt_id = ?".into());
            args.push(Box::new(prompt_id));
        }
        if let Some(title) = input.title.as_ref() {
            sets.push("title = ?".into());
            args.push(Box::new(title.clone()));
        }
        if let Some(notes) = input.notes.as_ref() {
            sets.push("notes = ?".into());
            args.push(Box::new(notes.clone()));
        }
        if let Some(source) = input.source.as_ref() {
            sets.push("source = ?".into());
            args.push(Box::new(source.clone()));
        }
        if let Some(favorite) = input.favorite {
            sets.push("favorite = ?".into());
            args.push(Box::new(if favorite { 1_i64 } else { 0_i64 }));
        }
        if let Some(nsfw) = input.nsfw {
            sets.push("nsfw = ?".into());
            args.push(Box::new(if nsfw { 1_i64 } else { 0_i64 }));
        }
        if sets.is_empty() {
            // 没有字段要更新 —— 与 Kotlin 一致地"当成没找到"（让空 body 立刻可见）
            return Ok(false);
        }
        args.push(Box::new(id));
        let sql = format!("UPDATE media_assets SET {} WHERE id = ?{}", sets.join(", "), args.len());
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        Ok(tx.execute(&sql, refs.as_slice())? > 0)
    })?;

    if !ok {
        return Err(ApiError::not_found(format!("产物 {id} 不存在或无字段需要更新")));
    }
    media::get(state, id)
}

/// 删产物：先删库（走事务），再删原件 / 缩略图 / 封面。
pub fn delete(state: &AppState, id: i64) -> Result<(), ApiError> {
    let Some(existing) = state
        .with(|conn| {
            conn.query_row(
                "SELECT stored_name FROM media_assets WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
        })?
    else {
        return Err(ApiError::not_found(format!("产物 {id} 不存在")));
    };

    let ok = state.write(|tx| {
        // 删之前先记住它属于哪个集合 —— 删完这一行就查不到了
        let collection_id: Option<i64> = tx
            .query_row(
                "SELECT collection_id FROM media_assets WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let affected = tx.execute("DELETE FROM media_assets WHERE id = ?1", params![id])?;
        if affected > 0 {
            // 标签计数要跟着变（`media_tags` 已被 CASCADE 清掉，R1 生效的前提下）
            repo::refresh_use_counts(tx)?;
            // 集合空了就把它一起删掉（2026-10-11）。
            // 不删的表现是"删到 0 张还留一个空组"，而且集合行会**永久累积**
            // （外键是 ON DELETE SET NULL，删产物不会带走集合行）。
            // ⚠ 注意这里删的是**整个集合**：用户把一组图逐张删完时，那个组就该消失。
            if let Some(cid) = collection_id {
                crate::media::collection::prune_if_empty(tx, cid)?;
            }
        }
        Ok(affected > 0)
    })?;
    if !ok {
        return Err(ApiError::not_found(format!("产物 {id} 不存在")));
    }

    // 磁盘清理：找不到文件 / 删不掉都**不影响响应**（库已经删干净了）
    let media_dir = state.media_dir().to_path_buf();
    if let Some(path) = media::resolve_media(&media_dir, &existing) {
        let _ = std::fs::remove_file(path);
    }
    if let Some(thumb_dir) = media_dir.parent() {
        let _ = std::fs::remove_file(thumb_dir.join("thumbs").join(format!("{id}.jpg")));
        let _ = std::fs::remove_file(thumb_dir.join("thumbs").join(format!("{id}.poster.png")));
    }
    Ok(())
}

/// `GET /api/stats` —— 顶部那几个数字（与 Kotlin `MediaRepo.stats` 同形）。
pub fn stats(state: &AppState) -> Result<serde_json::Value, ApiError> {
    state.with(|conn| {
        fn count(conn: &rusqlite::Connection, sql: &str) -> rusqlite::Result<i64> {
            conn.query_row(sql, [], |row| row.get(0))
        }
        fn grouped(conn: &rusqlite::Connection, sql: &str) -> rusqlite::Result<serde_json::Value> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            let mut map = serde_json::Map::new();
            for row in rows {
                let (kind, n) = row?;
                map.insert(kind, serde_json::json!(n));
            }
            Ok(serde_json::Value::Object(map))
        }
        Ok(serde_json::json!({
            "prompts": count(conn, "SELECT COUNT(*) FROM prompts")?,
            "media": count(conn, "SELECT COUNT(*) FROM media_assets")?,
            "tags": count(conn, "SELECT COUNT(*) FROM tags")?,
            "favoritePrompts": count(conn, "SELECT COUNT(*) FROM prompts WHERE favorite = 1")?,
            "favoriteMedia": count(conn, "SELECT COUNT(*) FROM media_assets WHERE favorite = 1")?,
            "byKind": grouped(conn, "SELECT kind, COUNT(*) FROM prompts GROUP BY kind")?,
            "byMediaKind": grouped(conn, "SELECT kind, COUNT(*) FROM media_assets GROUP BY kind")?,
        }))
    })
}
