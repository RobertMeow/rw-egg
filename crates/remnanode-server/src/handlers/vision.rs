use axum::{extract::State, response::Json};
use crate::state::AppState;
use md5::Digest;

fn success_response() -> Json<serde_json::Value> {
    Json(serde_json::json!({"response": {"success": true, "error": null}}))
}

fn error_response(message: impl Into<String>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"response": {"success": false, "error": message.into()}}))
}

fn extract_ips(body: &serde_json::Value) -> Vec<String> {
    if let Some(arr) = body.get("ips").and_then(|v| v.as_array()) {
        return arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    }
    if let Some(ip) = body.get("ip").and_then(|v| v.as_str()) {
        return vec![ip.to_string()];
    }
    Vec::new()
}

pub async fn block_ip(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let ips = extract_ips(&body);

    if ips.is_empty() {
        return error_response("missing ip(s)");
    }

    let mut xray = state.xray.write().await;
    match xray.router_client.as_mut() {
        Some(client) => {
            let mut any_ok = false;
            for ip in &ips {
                let digest = md5::Md5::digest(ip.as_bytes());
                let rule_tag = format!("block_{}", hex::encode(digest));
                if let Err(e) = client.add_rule(&rule_tag, std::slice::from_ref(ip), "BLOCK").await {
                    tracing::warn!("Failed to add block rule for {ip}: {e}");
                } else {
                    any_ok = true;
                }
            }
            if any_ok {
                success_response()
            } else {
                error_response("failed to add block rules")
            }
        }
        None => error_response("xray not connected"),
    }
}

pub async fn unblock_ip(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let ips = extract_ips(&body);

    if ips.is_empty() {
        return error_response("missing ip(s)");
    }

    let mut xray = state.xray.write().await;
    match xray.router_client.as_mut() {
        Some(client) => {
            let mut any_ok = false;
            for ip in &ips {
                let digest = md5::Md5::digest(ip.as_bytes());
                let rule_tag = format!("block_{}", hex::encode(digest));
                if let Err(e) = client.remove_rule(&rule_tag).await {
                    tracing::warn!("Failed to remove block rule for {ip}: {e}");
                } else {
                    any_ok = true;
                }
            }
            if any_ok {
                success_response()
            } else {
                error_response("failed to remove block rules")
            }
        }
        None => error_response("xray not connected"),
    }
}
