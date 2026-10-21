//! 角色卡门面（仓储与路由的真源在 [`crate::library::cards`]）。
//!
//! 保留这个名字，是因为 AI 工具与系统提示里说的都是"角色卡"：
//! `save_character_card` / `search_character_cards` / `/api/ai/character-cards`。
//! 实际实现与画风卡**共用同一份**，别再在这里复制一套。

pub use crate::library::cards::{
    CardDto as CharacterCardDto, CardInput as CharacterCardInput, MAX_LIBRARY_RESULTS,
    MAX_NAME_CHARS, MAX_PROMPT_CHARS, MAX_SEARCH,
};

use crate::library::cards::{self, CardKind};
use crate::web::routes::AppState;
use serde_json::Value;

pub fn tool_search(
    state: &AppState,
    arguments: &Value,
) -> Result<crate::tools::tools::ToolOutcome, crate::tools::tools::ToolFailure> {
    cards::tool_search(state, CardKind::Character, arguments)
}

pub fn tool_save(
    state: &AppState,
    arguments: &Value,
) -> Result<crate::tools::tools::ToolOutcome, crate::tools::tools::ToolFailure> {
    cards::tool_save(state, CardKind::Character, arguments)
}

#[cfg(test)]
mod tests;
