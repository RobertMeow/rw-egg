use axum::{extract::{State, Query}, response::Json};
use serde::Deserialize;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct TokenQuery {
    token: String,
}

pub async fn get_config(
    State(state): State<AppState>,
    Query(query): Query<TokenQuery>,
) -> Json<serde_json::Value> {
    if query.token != state.internal.token {
        return Json(serde_json::json!({"error": "invalid token"}));
    }

    let xray = state.xray.read().await;
    match &xray.config {
        Some(config) => Json(config.clone()),
        None => Json(serde_json::json!({})),
    }
}

pub async fn webhook(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> &'static str {
    // Process xray webhook (torrent detection, etc.)
    tracing::debug!("Received webhook: {:?}", body);

    let protocol = body.get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if protocol == "bittorrent" {
        let source = body.get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        tracing::info!("Torrent detected from {source}");

        let mut plugins = state.plugins.write().await;
        plugins.torrent_blocker.reports.push(
            remnanode_plugins::TorrentBlockerReport {
                ip: source.to_string(),
                blocked_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
                rule_tag: None,
            }
        );
    }

    "ok"
}
