//! 画风卡门面（仓储与路由的真源在 [`crate::library::cards`]）。
//!
//! 与角色卡**同等地位**：同一个仓储实现、同一套字段（名称 / Danbooru tag 版 /
//! 自然语言版 / 负面提示词 / 示例产物），只是表与工具名不同。
//! `save_style_card` / `search_style_cards` / `/api/ai/style-cards` 都走这里。

pub use crate::library::cards::{
    CardDto as StyleCardDto, CardInput as StyleCardInput, MAX_EXAMPLES, MAX_NAME_CHARS,
    MAX_PROMPT_CHARS,
};

use crate::library::cards::{self, CardKind};
use crate::web::routes::AppState;
use serde_json::Value;

pub fn tool_search(
    state: &AppState,
    arguments: &Value,
) -> Result<crate::tools::tools::ToolOutcome, crate::tools::tools::ToolFailure> {
    cards::tool_search(state, CardKind::Style, arguments)
}

pub fn tool_save(
    state: &AppState,
    arguments: &Value,
) -> Result<crate::tools::tools::ToolOutcome, crate::tools::tools::ToolFailure> {
    cards::tool_save(state, CardKind::Style, arguments)
}
