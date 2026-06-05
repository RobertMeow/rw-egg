use axum::{extract::State, response::Json, body::Bytes};
use crate::state::AppState;

fn parse_body(body: &Bytes) -> serde_json::Value {
    if body.is_empty() {
        return serde_json::json!({});
    }
    if let Ok(v) = serde_json::from_slice(body) {
        return v;
    }
    // Try zstd decompression
    if body.len() >= 4 && body[0..4] == [0x28, 0xb5, 0x2f, 0xfd] {
        if let Ok(decompressed) = zstd::decode_all(&body[..]) {
            if let Ok(v) = serde_json::from_slice(&decompressed) {
                return v;
            }
        }
    }
    // Fallback
    let s = String::from_utf8_lossy(body);
    if let Some(pos) = s.find(|c: char| c == '{' || c == '[') {
        if let Ok(v) = serde_json::from_str(&s[pos..]) {
            return v;
        }
    }
    serde_json::json!({})
}

pub async fn sync(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let parsed = parse_body(&body);
    tracing::info!("POST /node/plugin/sync plugin={:?}", parsed.get("plugin"));

    let plugin = parsed.get("plugin");

    if plugin.is_none() || plugin.unwrap().is_null() {
        tracing::info!("Plugin sync: clearing all plugins");
        let mut plugins = state.plugins.write().await;
        plugins.torrent_blocker.enabled = false;
        plugins.torrent_blocker.include_rule_tags.clear();
        return Json(serde_json::json!({"response": {"accepted": true}}));
    }

    let plugin_obj = plugin.unwrap();
    let config = plugin_obj.get("config").cloned().unwrap_or(serde_json::json!({}));
    let _uuid = plugin_obj.get("uuid").and_then(|v| v.as_str()).unwrap_or("");
    let name = plugin_obj.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");

    tracing::info!("Plugin sync: name={name}");

    let mut plugins = state.plugins.write().await;
    plugins.sync(&config);

    tracing::info!("Plugin sync done: torrent_blocker.enabled={}", plugins.torrent_blocker.enabled);

    Json(serde_json::json!({"response": {"accepted": true}}))
}

pub async fn torrent_blocker_collect(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    tracing::info!("POST /node/plugin/torrent-blocker/collect");
    let mut plugins = state.plugins.write().await;
    let reports = plugins.collect_torrent_reports();
    Json(serde_json::json!({"response": {"reports": reports}}))
}

pub async fn nftables_block_ips(
    State(_state): State<AppState>,
    _body: Bytes,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"response": {"accepted": true}}))
}

pub async fn nftables_unblock_ips(
    State(_state): State<AppState>,
    _body: Bytes,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"response": {"accepted": true}}))
}

pub async fn nftables_recreate_tables(
    State(_state): State<AppState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"response": {"accepted": true}}))
}
