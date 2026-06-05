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

pub async fn add_user(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    tracing::info!("POST /node/handler/add-user");
    let body = parse_body(&body);
    let data = match body.get("data").and_then(|v| v.as_array()) {
        Some(d) => d,
        None => return Json(serde_json::json!({"response": {"message": "missing data"}})),
    };
    let hash_data = body.get("hashData").cloned().unwrap_or(serde_json::json!({}));
    let username = data[0].get("username").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let uuid = hash_data.get("vlessUuid").and_then(|v| v.as_str()).unwrap_or("").to_string();
    tracing::info!("add_user: username={username}");

    let inbound_tags: Vec<String> = {
        let xray = state.xray.read().await;
        xray.xtls_config_inbounds.iter().cloned().collect()
    };

    // Remove user from all inbounds via gRPC
    let mut xray = state.xray.write().await;
    if let Some(client) = xray.handler_client.as_mut() {
        for tag in &inbound_tags {
            let _ = client.alter_inbound_remove_user(tag, &username).await;
        }
    }

    // Add user to each inbound via gRPC, collect results
    let mut successes: Vec<(String, String)> = Vec::new(); // (tag, uuid)
    let mut any_ok = false;

    if let Some(client) = xray.handler_client.as_mut() {
        for item in data {
            let tag = item.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let user_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let user = build_user(user_type, &username, item, &uuid);
            if let Some(user) = user {
                match client.alter_inbound_add_user(&tag, user).await {
                    Ok(_) => {
                        any_ok = true;
                        successes.push((tag.clone(), uuid.clone()));
                    }
                    Err(e) => tracing::warn!("Failed to add user to {tag}: {e}"),
                }
            }
        }
    }

    // Apply state mutations
    for (tag, uid) in &successes {
        xray.add_user_to_inbound(tag, uid);
    }

    Json(serde_json::json!({"response": {"isAdded": any_ok}}))
}

pub async fn remove_user(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    tracing::info!("POST /node/handler/remove-user");
    let body = parse_body(&body);
    let username = body.get("username").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let hash_data = body.get("hashData").cloned().unwrap_or(serde_json::json!({}));
    let uuid = hash_data.get("vlessUuid").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let inbound_tags: Vec<String> = {
        let xray = state.xray.read().await;
        xray.xtls_config_inbounds.iter().cloned().collect()
    };

    let mut xray = state.xray.write().await;
    let mut successes: Vec<(String, String)> = Vec::new();

    if let Some(client) = xray.handler_client.as_mut() {
        for tag in &inbound_tags {
            match client.alter_inbound_remove_user(tag, &username).await {
                Ok(_) => successes.push((tag.clone(), uuid.clone())),
                Err(e) => tracing::warn!("Failed to remove user from {tag}: {e}"),
            }
        }
    }

    for (tag, uid) in &successes {
        xray.remove_user_from_inbound(tag, uid);
    }

    Json(serde_json::json!({"response": {"isRemoved": !successes.is_empty()}}))
}

pub async fn add_users(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let affected_tags: Vec<String> = body.get("affectedInboundTags")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let users: Vec<serde_json::Value> = body.get("users")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    {
        let mut xray = state.xray.write().await;
        for tag in &affected_tags {
            xray.add_xtls_config_inbound(tag.clone());
        }
    }

    let inbound_tags: Vec<String> = {
        let xray = state.xray.read().await;
        xray.xtls_config_inbounds.iter().cloned().collect()
    };

    let mut xray = state.xray.write().await;
    // Collect all mutations to apply after gRPC calls
    let mut pending_mutations: Vec<(String, String, bool)> = Vec::new(); // (tag, uuid, is_add)

    if let Some(client) = xray.handler_client.as_mut() {
        for user in &users {
            let user_id = user.get("userData").and_then(|v| v.get("userId")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let vless_uuid = user.get("userData").and_then(|v| v.get("vlessUuid")).and_then(|v| v.as_str()).unwrap_or("").to_string();

            // Remove from all inbounds
            for tag in &inbound_tags {
                let _ = client.alter_inbound_remove_user(tag, &user_id).await;
            }

            // Queue remove mutations
            for tag in &inbound_tags {
                pending_mutations.push((tag.clone(), vless_uuid.clone(), false));
            }

            // Add to each inbound
            if let Some(items) = user.get("inboundData").and_then(|v| v.as_array()) {
                for item in items {
                    let tag = item.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let user_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let trojan_pwd = user.get("userData").and_then(|v| v.get("trojanPassword")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let ss_pwd = user.get("userData").and_then(|v| v.get("ssPassword")).and_then(|v| v.as_str()).unwrap_or("").to_string();

                    let acc = build_user_extended(user_type, &user_id, item, &vless_uuid, &trojan_pwd, &ss_pwd);
                    if let Some(acc) = acc {
                        if client.alter_inbound_add_user(&tag, acc).await.is_ok() {
                            pending_mutations.push((tag, vless_uuid.clone(), true));
                        }
                    }
                }
            }
        }
    }

    // Apply state mutations
    for (tag, uid, is_add) in &pending_mutations {
        if *is_add {
            xray.add_user_to_inbound(tag, uid);
        } else {
            xray.remove_user_from_inbound(tag, uid);
        }
    }

    Json(serde_json::json!({"response": {}}))
}

pub async fn remove_users(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let users: Vec<serde_json::Value> = body.get("users")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let inbound_tags: Vec<String> = {
        let xray = state.xray.read().await;
        xray.xtls_config_inbounds.iter().cloned().collect()
    };

    let mut xray = state.xray.write().await;
    if let Some(client) = xray.handler_client.as_mut() {
        let mut to_remove: Vec<(String, String)> = Vec::new();
        for user in &users {
            let user_id = user.get("userId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let hash_uuid = user.get("hashUuid").and_then(|v| v.as_str()).unwrap_or("").to_string();
            for tag in &inbound_tags {
                let _ = client.alter_inbound_remove_user(tag, &user_id).await;
                to_remove.push((tag.clone(), hash_uuid.clone()));
            }
        }
        for (tag, uid) in &to_remove {
            xray.remove_user_from_inbound(tag, uid);
        }
    }

    Json(serde_json::json!({"response": {}}))
}

pub async fn get_inbound_users(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let tag = body.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut xray = state.xray.write().await;
    match xray.handler_client.as_mut() {
        Some(client) => match client.get_inbound_users(&tag).await {
            Ok(_) => Json(serde_json::json!({"response": {}})),
            Err(e) => Json(serde_json::json!({"response": {"message": e}})),
        },
        None => Json(serde_json::json!({"response": {"users": []}})),
    }
}

pub async fn get_inbound_users_count(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let tag = body.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut xray = state.xray.write().await;
    match xray.handler_client.as_mut() {
        Some(client) => match client.get_inbound_users_count(&tag).await {
            Ok(count) => Json(serde_json::json!({"response": {"count": count}})),
            Err(_) => Json(serde_json::json!({"response": {"count": 0}})),
        },
        None => Json(serde_json::json!({"response": {"count": 0}})),
    }
}

pub async fn drop_users_connections(
    State(_state): State<AppState>,
    _body: axum::body::Bytes,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"response": {"isDropped": false}}))
}

pub async fn drop_ips(
    State(_state): State<AppState>,
    _body: axum::body::Bytes,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"response": {"isDropped": false}}))
}

fn build_user(user_type: &str, username: &str, item: &serde_json::Value, uuid: &str) -> Option<remnanode_proto::xray::common::protocol::User> {
    match user_type {
        "vless" => Some(remnanode_xray::grpc::handler::accounts::vless_user(
            username,
            item.get("uuid").and_then(|v| v.as_str()).unwrap_or(uuid),
            item.get("flow").and_then(|v| v.as_str()).unwrap_or(""),
        )),
        "trojan" => Some(remnanode_xray::grpc::handler::accounts::trojan_user(
            username,
            item.get("password").and_then(|v| v.as_str()).unwrap_or(""),
        )),
        "shadowsocks" => Some(remnanode_xray::grpc::handler::accounts::shadowsocks_user(
            username,
            item.get("password").and_then(|v| v.as_str()).unwrap_or(""),
        )),
        "shadowsocks22" => Some(remnanode_xray::grpc::handler::accounts::shadowsocks_2022_user(
            username,
            item.get("password").and_then(|v| v.as_str()).unwrap_or(""),
        )),
        "hysteria" => Some(remnanode_xray::grpc::handler::accounts::hysteria_user(
            username,
            item.get("password").and_then(|v| v.as_str()).unwrap_or(uuid),
        )),
        _ => None,
    }
}

fn build_user_extended(user_type: &str, user_id: &str, item: &serde_json::Value, vless_uuid: &str, trojan_pwd: &str, ss_pwd: &str) -> Option<remnanode_proto::xray::common::protocol::User> {
    match user_type {
        "vless" => Some(remnanode_xray::grpc::handler::accounts::vless_user(
            user_id, vless_uuid,
            item.get("flow").and_then(|v| v.as_str()).unwrap_or(""),
        )),
        "trojan" => Some(remnanode_xray::grpc::handler::accounts::trojan_user(user_id, trojan_pwd)),
        "shadowsocks" => Some(remnanode_xray::grpc::handler::accounts::shadowsocks_user(user_id, ss_pwd)),
        "shadowsocks22" => {
            let key_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, ss_pwd.as_bytes());
            Some(remnanode_xray::grpc::handler::accounts::shadowsocks_2022_user(user_id, &key_b64))
        }
        "hysteria" => Some(remnanode_xray::grpc::handler::accounts::hysteria_user(user_id, vless_uuid)),
        _ => None,
    }
}
