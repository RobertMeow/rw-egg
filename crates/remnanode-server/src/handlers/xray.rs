use axum::{extract::State, response::Json, body::Bytes};
use crate::state::AppState;
use std::time::Duration;

fn parse_body(body: &Bytes) -> serde_json::Value {
    if body.is_empty() {
        return serde_json::json!({});
    }

    // Try raw JSON first
    if let Ok(v) = serde_json::from_slice(body) {
        return v;
    }

    // Try zstd decompression (panel sends compressed bodies)
    if body.len() >= 4 && body[0..4] == [0x28, 0xb5, 0x2f, 0xfd] {
        if let Ok(decompressed) = zstd::decode_all(&body[..]) {
            if let Ok(v) = serde_json::from_slice(&decompressed) {
                return v;
            }
        }
    }

    // Fallback: look for JSON in raw bytes
    let s = String::from_utf8_lossy(body);
    if let Some(pos) = s.find('{') {
        if let Ok(v) = serde_json::from_str(&s[pos..]) {
            return v;
        }
    }
    serde_json::json!({})
}

pub async fn start(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    tracing::info!("POST /node/xray/start body_len={}", body.len());
    let body = parse_body(&body);

    let xray_config = body.get("xrayConfig")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    let is_torrent_blocker_enabled = body.get("torrentBlockerState")
        .and_then(|v| v.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let torrent_include_tags: std::collections::HashSet<String> = body.get("torrentBlockerState")
        .and_then(|v| v.get("includeRuleTags"))
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let force_restart = body.get("internals")
        .and_then(|v| v.get("forceRestart"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    tracing::info!("Xray start: torrent_blocker={is_torrent_blocker_enabled}, force_restart={force_restart}");

    let mtls = state.mtls_certs.clone();
    let full_config = remnanode_config::xray_config::generate_api_config(
        &xray_config,
        state.env.xtls_api_port,
        state.env.xray_proxy_port,
        &mtls,
        is_torrent_blocker_enabled,
        &torrent_include_tags,
        &state.internal.socket_path,
        &state.internal.token,
    );

    tracing::info!("Panel xrayConfig: {}", serde_json::to_string(&xray_config).unwrap_or_default());

    let hashes = body.get("internals")
        .and_then(|v| v.get("hashes"))
        .cloned()
        .unwrap_or(serde_json::json!({}));

    {
        let mut xray = state.xray.write().await;
        xray.extract_users_from_config(&hashes, &full_config);
        xray.config = Some(full_config);
        // Полный конфиг от панели уже содержит всех юзеров
        xray.journal.clear();
        xray.journal_overflow = false;
    }

    {
        let mut xray = state.xray.write().await;

        if let Some(ref mut child) = xray.process {
            tracing::info!("Stopping existing xray process");
            let _ = crate::xray_process::stop_xray(child).await;
            xray.process = None;
        }

        match crate::xray_process::start_xray(
            &state.internal.socket_path,
            &state.internal.token,
        ).await {
            Ok(child) => {
                let pid = child.id();
                tracing::info!("Xray process started (PID {pid:?})");
                xray.process = Some(child);
            }
            Err(e) => {
                tracing::error!("Failed to start xray: {e}");
                return Json(serde_json::json!({
                    "response": { "message": e }
                }));
            }
        }
    }

    // gRPC retry loop — acquire/release write lock per iteration
    // to avoid deadlock: xray needs read lock to fetch config from internal server
    for attempt in 0..20 {
        tokio::time::sleep(Duration::from_secs(2)).await;

        let result = {
            let mut xray = state.xray.write().await;

            // Check if xray process is still alive
            if let Some(ref mut child) = xray.process {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        tracing::error!("Xray process exited prematurely: {status}");
                        xray.process = None;
                        return Json(serde_json::json!({
                            "response": { "message": format!("Xray exited: {status}") }
                        }));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!("Failed to check xray status: {e}");
                    }
                }
            } else {
                tracing::error!("Xray process lost");
                return Json(serde_json::json!({
                    "response": { "message": "Xray process lost" }
                }));
            }

            xray.connect_grpc(state.env.xtls_api_port, &state.mtls_certs).await
        };
        // write lock released here — internal server can serve config

        match result {
            Ok(_) => {
                tracing::info!("gRPC connected after {} attempts", attempt + 1);
                break;
            }
            Err(e) => {
                tracing::warn!("gRPC attempt {}/20 failed: {e}", attempt + 1);
                if attempt == 19 {
                    tracing::error!("gRPC connection failed after 20 attempts");
                    return Json(serde_json::json!({
                        "response": { "message": format!("gRPC connection failed: {e}") }
                    }));
                }
            }
        }
    }

    tracing::info!("Xray started successfully");
    Json(serde_json::json!({
        "response": {
            "isStarted": true,
            "version": xray_version(),
            "error": null,
            "nodeInformation": {
                "version": NODE_VERSION
            },
            "system": {
                "info": crate::handlers::stats::system_info(),
                "stats": crate::handlers::stats::read_system_stats()
            }
        }
    }))
}

pub async fn stop(State(state): State<AppState>) -> Json<serde_json::Value> {
    tracing::info!("GET /node/xray/stop");
    let mut xray = state.xray.write().await;

    if let Some(ref mut child) = xray.process {
        match crate::xray_process::stop_xray(child).await {
            Ok(_) => {
                tracing::info!("Xray stopped");
                xray.process = None;
                xray.handler_client = None;
                xray.stats_client = None;
                xray.router_client = None;
                Json(serde_json::json!({"response": {"isStopped": true}}))
            }
            Err(e) => {
                tracing::error!("Failed to stop xray: {e}");
                Json(serde_json::json!({"response": {"isStopped": false, "message": e}}))
            }
        }
    } else {
        Json(serde_json::json!({"response": {"isStopped": true}}))
    }
}

pub async fn healthcheck(State(state): State<AppState>) -> Json<serde_json::Value> {
    let xray = state.xray.read().await;

    let xray_running = xray.process.as_ref()
        .and_then(|c| c.id())
        .is_some();

    let grpc_ok = xray.stats_client.is_some();

    tracing::debug!("Healthcheck: xray_running={xray_running}, grpc_ok={grpc_ok}");

    Json(serde_json::json!({
        "response": {
            "isAlive": true,
            "xrayInternalStatusCached": grpc_ok,
            "xrayVersion": xray_version(),
            "nodeVersion": NODE_VERSION
        }
    }))
}

/// Версия ноды для панели. Без суффиксов: панель сравнивает по semver (>= 2.7.0),
/// а «2.7.0-rs» считается пре-релизом и отклоняется как устаревшая.
const NODE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Версия Xray без «v»: из `rw-core version` («Xray 26.3.27 (...)»), иначе из XRAY_CORE_VERSION.
fn xray_version() -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            std::process::Command::new("/home/container/runtime/bin/rw-core")
                .arg("version")
                .output()
                .ok()
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .split_whitespace()
                        .nth(1)
                        .map(|v| v.trim_start_matches('v').to_string())
                })
                .or_else(|| {
                    std::env::var("XRAY_CORE_VERSION")
                        .ok()
                        .map(|v| v.trim_start_matches('v').to_string())
                })
        })
        .clone()
}
