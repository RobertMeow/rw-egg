use axum::{extract::State, response::Json, body::Bytes};
use crate::state::AppState;

fn parse_body(body: &Bytes) -> serde_json::Value {
    if body.is_empty() {
        return serde_json::json!({});
    }
    if let Ok(v) = serde_json::from_slice(body) {
        return v;
    }
    let s = String::from_utf8_lossy(body);
    if let Some(pos) = s.find('{') {
        if let Ok(v) = serde_json::from_str(&s[pos..]) {
            return v;
        }
    }
    serde_json::json!({})
}

pub async fn get_user_online_status(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let users: Vec<String> = body.get("users")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let mut result = serde_json::Map::new();
    let mut xray = state.xray.write().await;

    if let Some(client) = xray.stats_client.as_mut() {
        for user_id in &users {
            let name = format!("user>>>{user_id}>>>online");
            match client.get_user_online(&name, false).await {
                Ok(online) => { result.insert(user_id.clone(), serde_json::Value::Bool(online)); }
                Err(_) => { result.insert(user_id.clone(), serde_json::Value::Bool(false)); }
            }
        }
    }

    Json(serde_json::json!({"response": result}))
}

pub async fn get_users_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let users: Vec<String> = body.get("users")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let mut result = Vec::new();
    let mut xray = state.xray.write().await;

    if let Some(client) = xray.stats_client.as_mut() {
        for user_id in &users {
            let uplink_pattern = format!("user>>>{user_id}>>>traffic>>>uplink");
            let downlink_pattern = format!("user>>>{user_id}>>>traffic>>>downlink");

            let uplink_value = client.query_stats(&uplink_pattern, false).await
                .ok()
                .and_then(|v| v.as_object().and_then(|o| o.values().next().and_then(|n| n.as_i64())))
                .unwrap_or(0);
            let downlink_value = client.query_stats(&downlink_pattern, false).await
                .ok()
                .and_then(|v| v.as_object().and_then(|o| o.values().next().and_then(|n| n.as_i64())))
                .unwrap_or(0);

            result.push(serde_json::json!({
                "username": user_id,
                "uplink": uplink_value,
                "downlink": downlink_value,
            }));
        }
    }

    Json(serde_json::json!({"response": result}))
}

pub async fn get_system_stats(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut xray = state.xray.write().await;
    let sys_stats = match xray.stats_client.as_mut() {
        Some(client) => client.get_sys_stats().await.ok(),
        None => None,
    };

    Json(serde_json::json!({
        "response": {
            "xrayStats": sys_stats,
            "nodePort": state.env.node_port,
        }
    }))
}

pub async fn get_inbound_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let tag = body.get("tag").and_then(|v| v.as_str()).unwrap_or("");
    let pattern = format!("inbound>>>{tag}>>>traffic>>>*");
    stats_query(&state, &pattern).await
}

pub async fn get_outbound_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let tag = body.get("tag").and_then(|v| v.as_str()).unwrap_or("");
    let pattern = format!("outbound>>>{tag}>>>traffic>>>*");
    stats_query(&state, &pattern).await
}

pub async fn get_all_outbounds_stats(
    State(state): State<AppState>,
    _body: Bytes,
) -> Json<serde_json::Value> {
    stats_query(&state, "outbound>>>traffic>>>*").await
}

pub async fn get_all_inbounds_stats(
    State(state): State<AppState>,
    _body: Bytes,
) -> Json<serde_json::Value> {
    stats_query(&state, "inbound>>>traffic>>>*").await
}

pub async fn get_combined_stats(
    State(state): State<AppState>,
    _body: Bytes,
) -> Json<serde_json::Value> {
    stats_query(&state, ">>>traffic>>>*").await
}

async fn stats_query(state: &AppState, pattern: &str) -> Json<serde_json::Value> {
    let mut xray = state.xray.write().await;
    let result = match xray.stats_client.as_mut() {
        Some(client) => client.query_stats(pattern, false).await.ok(),
        None => None,
    };
    Json(serde_json::json!({"response": result.unwrap_or(serde_json::json!({}))}))
}

pub async fn get_user_ip_list(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let user_id = body.get("username").and_then(|v| v.as_str()).unwrap_or("");
    let name = format!("user>>>{user_id}>>>online");
    let mut xray = state.xray.write().await;

    let result = match xray.stats_client.as_mut() {
        Some(client) => client.get_online_ip_list(&name, true).await.ok(),
        None => None,
    };

    Json(serde_json::json!({"response": result.unwrap_or(serde_json::json!({"ips": {}}))}))
}

pub async fn get_users_ip_list(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"response": []}))
}
