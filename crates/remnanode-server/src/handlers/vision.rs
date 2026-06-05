use axum::{extract::State, response::Json};
use crate::state::AppState;
use md5::Digest;

pub async fn block_ip(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let ips: Vec<String> = body.get("ips")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    if ips.is_empty() {
        return Json(serde_json::json!({"response": {}}));
    }

    let mut xray = state.xray.write().await;
    match xray.router_client.as_mut() {
        Some(client) => {
            for ip in &ips {
                let digest = md5::Md5::digest(ip.as_bytes());
                let rule_tag = format!("block_{}", hex::encode(digest));
                if let Err(e) = client.add_rule(&rule_tag, &ips, "BLOCK").await {
                    tracing::warn!("Failed to add block rule for {ip}: {e}");
                }
            }
            Json(serde_json::json!({"response": {}}))
        }
        None => Json(serde_json::json!({"response": {"message": "xray not connected"}})),
    }
}

pub async fn unblock_ip(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let ips: Vec<String> = body.get("ips")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let mut xray = state.xray.write().await;
    match xray.router_client.as_mut() {
        Some(client) => {
            for ip in &ips {
                let digest = md5::Md5::digest(ip.as_bytes());
                let rule_tag = format!("block_{}", hex::encode(digest));
                if let Err(e) = client.remove_rule(&rule_tag).await {
                    tracing::warn!("Failed to remove block rule for {ip}: {e}");
                }
            }
            Json(serde_json::json!({"response": {}}))
        }
        None => Json(serde_json::json!({"response": {"message": "xray not connected"}})),
    }
}
