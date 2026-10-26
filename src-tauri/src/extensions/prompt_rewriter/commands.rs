//! Synchronous extension commands. No lock crosses an async boundary.
use super::{Config, Preview, PromptRewriter, Route, Status, WireFormat};
use serde_json::Value;
use tauri::State;

#[tauri::command]
pub fn prompt_rewriter_get(state: State<'_, PromptRewriter>) -> Result<Status, String> {
    state.status()
}

#[tauri::command]
pub fn prompt_rewriter_save(
    state: State<'_, PromptRewriter>,
    config: Config,
) -> Result<Status, String> {
    state.save(config)
}

#[tauri::command]
pub fn prompt_rewriter_reload(state: State<'_, PromptRewriter>) -> Result<Status, String> {
    state.reload()
}

#[tauri::command]
pub fn prompt_rewriter_refresh_sources(state: State<'_, PromptRewriter>) -> Result<Status, String> {
    state.refresh_sources()
}

#[tauri::command]
pub fn prompt_rewriter_capture(
    state: State<'_, PromptRewriter>,
    route: Route,
) -> Result<Status, String> {
    state.capture(route)
}

#[tauri::command]
pub fn prompt_rewriter_preview(
    state: State<'_, PromptRewriter>,
    config: Config,
    route: Route,
    format: WireFormat,
    body: Value,
) -> Result<Preview, String> {
    state.preview(config, route, format, body)
}
