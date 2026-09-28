use axum::{extract::State, response::Json, body::Bytes};
use crate::state::AppState;
use std::collections::HashMap;

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

fn get_reset(body: &serde_json::Value) -> bool {
    body.get("reset").and_then(|v| v.as_bool()).unwrap_or(false)
}

fn format_last_seen(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

pub async fn get_user_online_status(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let username = body.get("username").and_then(|v| v.as_str()).unwrap_or("");

    // Online detection is traffic-recency based (the accumulator), so it works
    // without CAP_NET_ADMIN. xray's native `>>>online` stat requires kernel
    // connection tracking and is unavailable in a non-root Pterodactyl container.
    let is_online = state
        .traffic
        .as_ref()
        .map(|shared| crate::traffic::is_online(shared, username))
        .unwrap_or(false);

    Json(serde_json::json!({"response": {"isOnline": is_online}}))
}

pub async fn get_users_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let reset = get_reset(&body);

    // Served from the node-side traffic accumulator (source of truth), not
    // directly from xray. The node polls xray's counters with reset:true on a
    // fixed cadence, so this returns the delta since the panel's last call
    // (reset=true) or cumulative totals (reset=false) — same contract as before,
    // but robust against xray/container restarts.
    let users = match state.traffic.as_ref() {
        Some(shared) => crate::traffic::users_delta(shared, reset)
            .into_iter()
            .map(|(username, uplink, downlink)| {
                serde_json::json!({
                    "username": username,
                    "uplink": uplink,
                    "downlink": downlink,
                })
            })
            .collect(),
        None => Vec::new(),
    };

    Json(serde_json::json!({"response": {"users": users}}))
}

pub async fn get_system_stats(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut xray = state.xray.write().await;
    let xray_info = match xray.stats_client.as_mut() {
        Some(client) => {
            let mut result: Option<serde_json::Value> = None;
            for attempt in 1..=3 {
                match client.get_sys_stats().await {
                    Ok(info) => {
                        tracing::info!("get_sys_stats succeeded on attempt {attempt}");
                        result = Some(info);
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("get_sys_stats attempt {attempt}/3 failed: {e}");
                        if attempt == 3 {
                            tracing::error!("get_sys_stats failed after 3 attempts");
                        } else {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                }
            }
            result
        }
        None => {
            tracing::warn!("get_system_stats: stats_client is None");
            None
        }
    };

    let reports_count = {
        let plugins = state.plugins.read().await;
        plugins.torrent_blocker.reports.len() as i64
    };

    let interface = state
        .network_stats
        .as_ref()
        .and_then(|ns| ns.lock().ok())
        .and_then(|guard| guard.get_default_rate())
        .map(|rate| {
            serde_json::json!({
                "interface": rate.interface,
                "rxBytesPerSec": rate.rx_bytes_per_sec,
                "txBytesPerSec": rate.tx_bytes_per_sec,
                "rxTotal": rate.rx_total,
                "txTotal": rate.tx_total,
            })
        });

    let system_stats = crate::system_stats::collect_system_stats_with_interface(interface);

    Json(serde_json::json!({
        "response": {
            "xrayInfo": xray_info,
            "plugins": {
                "torrentBlocker": {
                    "reportsCount": reports_count
                }
            },
            "system": {
                "stats": system_stats
            }
        }
    }))
}

pub async fn get_inbound_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let tag = body.get("tag").and_then(|v| v.as_str()).unwrap_or("");
    let reset = get_reset(&body);
    let (uplink, downlink) = traffic_for(&state, &format!("inbound>>>{tag}>>>"), reset).await;
    Json(serde_json::json!({
        "response": {
            "inbound": tag,
            "downlink": downlink,
            "uplink": uplink,
        }
    }))
}

pub async fn get_outbound_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let tag = body.get("tag").and_then(|v| v.as_str()).unwrap_or("");
    let reset = get_reset(&body);
    let (uplink, downlink) = traffic_for(&state, &format!("outbound>>>{tag}>>>"), reset).await;
    Json(serde_json::json!({
        "response": {
            "outbound": tag,
            "downlink": downlink,
            "uplink": uplink,
        }
    }))
}

pub async fn get_all_inbounds_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let reset = get_reset(&parse_body(&body));
    let inbounds = traffic_list(&state, "inbound>>>", "inbound", reset).await;
    Json(serde_json::json!({ "response": { "inbounds": inbounds } }))
}

pub async fn get_all_outbounds_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let reset = get_reset(&parse_body(&body));
    let outbounds = traffic_list(&state, "outbound>>>", "outbound", reset).await;
    Json(serde_json::json!({ "response": { "outbounds": outbounds } }))
}

pub async fn get_combined_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let reset = get_reset(&parse_body(&body));
    let inbounds = traffic_list(&state, "inbound>>>", "inbound", reset).await;
    let outbounds = traffic_list(&state, "outbound>>>", "outbound", reset).await;
    Json(serde_json::json!({
        "response": {
            "inbounds": inbounds,
            "outbounds": outbounds,
        }
    }))
}

/// Summed (uplink, downlink) for a single inbound/outbound tag, queried from
/// xray with a bare `prefix` (e.g. `"inbound>>>TAG>>>"`). xray does substring
/// matching, so no wildcard is used.
async fn traffic_for(state: &AppState, prefix: &str, reset: bool) -> (i64, i64) {
    let mut xray = state.xray.write().await;
    match xray.stats_client.as_mut() {
        Some(client) => match client.get_traffic_stats(prefix, reset).await {
            Ok(entries) => entries.into_iter().fold((0, 0), |(u, d), (_, eu, ed)| (u + eu, d + ed)),
            Err(e) => {
                tracing::warn!("get_traffic_stats({prefix}) failed: {e}");
                (0, 0)
            }
        },
        None => (0, 0),
    }
}

/// List of `{<key>, downlink, uplink}` objects for every tag under `prefix`
/// (e.g. `"inbound>>>"`). `key` is `"inbound"` or `"outbound"` — the JSON field
/// name the panel expects, which can't be expressed as a `json!` literal.
async fn traffic_list(state: &AppState, prefix: &str, key: &str, reset: bool) -> Vec<serde_json::Value> {
    let mut xray = state.xray.write().await;
    match xray.stats_client.as_mut() {
        Some(client) => match client.get_traffic_stats(prefix, reset).await {
            Ok(entries) => entries
                .into_iter()
                .map(|(tag, uplink, downlink)| {
                    let mut obj = serde_json::Map::new();
                    obj.insert(key.to_string(), serde_json::Value::String(tag));
                    obj.insert("downlink".to_string(), downlink.into());
                    obj.insert("uplink".to_string(), uplink.into());
                    serde_json::Value::Object(obj)
                })
                .collect(),
            Err(e) => {
                tracing::warn!("get_traffic_stats({prefix}) failed: {e}");
                Vec::new()
            }
        },
        None => Vec::new(),
    }
}

pub async fn get_user_ip_list(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let user_id = body.get("userId").and_then(|v| v.as_str()).unwrap_or("");
    let name = format!("user>>>{user_id}>>>online");

    let mut xray = state.xray.write().await;
    let ips = match xray.stats_client.as_mut() {
        Some(client) => client.get_online_ip_list(&name, true).await.unwrap_or_default(),
        None => HashMap::new(),
    };

    let ips: Vec<serde_json::Value> = ips
        .into_iter()
        .map(|(ip, last_seen)| {
            serde_json::json!({
                "ip": ip,
                "lastSeen": format_last_seen(last_seen),
            })
        })
        .collect();

    Json(serde_json::json!({"response": {"ips": ips}}))
}

pub async fn get_users_ip_list(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut xray = state.xray.write().await;

    let online_users = match xray.stats_client.as_mut() {
        Some(client) => client.get_all_online_users().await.unwrap_or_default(),
        None => Vec::new(),
    };

    let mut users = Vec::new();

    if let Some(client) = xray.stats_client.as_mut() {
        for raw in online_users {
            // raw format: "user>>>EMAIL>>>online"
            let email = raw.split(">>>").nth(1).unwrap_or(&raw).to_string();
            let name = format!("user>>>{email}>>>online");
            let ips = client.get_online_ip_list(&name, true).await.unwrap_or_default();

            let ips: Vec<serde_json::Value> = ips
                .into_iter()
                .map(|(ip, last_seen)| {
                    serde_json::json!({
                        "ip": ip,
                        "lastSeen": format_last_seen(last_seen),
                    })
                })
                .collect();

            if !ips.is_empty() {
                users.push(serde_json::json!({
                    "userId": email,
                    "ips": ips,
                }));
            }
        }
    }

    Json(serde_json::json!({"response": {"users": users}}))
}
