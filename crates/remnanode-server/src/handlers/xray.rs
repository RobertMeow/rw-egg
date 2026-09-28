use axum::{extract::State, response::Json, body::Bytes};
use crate::state::AppState;

/// Normalize an xray version string to a plain semver like "26.3.27",
/// matching upstream's `semver.valid(semver.coerce(...))` behaviour.
fn normalize_xray_version(raw: &str) -> Option<String> {
    // Strip optional leading 'v'/'V' and take the first dotted numeric sequence.
    let stripped = raw.trim_start_matches(['v', 'V']);
    let mut result = String::new();
    let mut prev_was_digit = false;
    for ch in stripped.chars() {
        if ch.is_ascii_digit() {
            result.push(ch);
            prev_was_digit = true;
        } else if ch == '.' && prev_was_digit {
            result.push(ch);
            prev_was_digit = false;
        } else if !result.is_empty() {
            break;
        }
    }

    if result.is_empty() || result.ends_with('.') {
        None
    } else {
        Some(result)
    }
}

fn xray_version(env: &remnanode_config::EnvConfig) -> Option<String> {
    normalize_xray_version(&env.xray_core_version)
}

/// If `panel_config` has a Hysteria inbound, make sure a Let's Encrypt cert
/// is on disk for it before xray starts. A failure here is logged and
/// swallowed rather than aborting the start — xray will simply fail to bind
/// the Hysteria inbound and the panel/logs will surface that, same as any
/// other misconfiguration.
async fn ensure_hysteria_cert(env: &remnanode_config::EnvConfig, panel_config: &serde_json::Value) {
    let Some(domain) = remnanode_config::acme::find_hysteria_domain(panel_config) else {
        return;
    };

    let Some(cf_token) = env.cf_token.as_deref() else {
        tracing::warn!(
            "Panel config has a Hysteria inbound for {domain} but CF_TOKEN is not set; \
             its TLS certificate cannot be issued"
        );
        return;
    };

    if let Err(e) = remnanode_config::acme::ensure_cert(&domain, cf_token).await {
        tracing::error!("Failed to ensure Hysteria certificate for {domain}: {e}");
    }
}

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

    ensure_hysteria_cert(&state.env, &xray_config).await;

    let mtls = state.mtls_certs.clone();
    let full_config = remnanode_config::xray_config::generate_api_config(
        &xray_config,
        state.env.xtls_api_port,
        state.env.xray_proxy_port,
        state.env.node_port,
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

    // Decide whether a full restart is required. If xray is already online and
    // the configuration hashes have not changed, we can skip the restart to avoid
    // dropping active connections.
    let should_restart = {
        let mut xray = state.xray.write().await;

        if force_restart {
            tracing::warn!("Force restart requested");
            true
        } else if xray.process.is_none() || xray.stats_client.is_none() {
            tracing::info!("Xray is not online - restart required");
            true
        } else {
            // Check gRPC health and compare hashes.
            let grpc_healthy = match xray.stats_client.as_mut() {
                Some(client) => client.get_sys_stats().await.is_ok(),
                None => false,
            };

            if !grpc_healthy {
                tracing::warn!("Xray Core health check failed, restarting...");
                true
            } else {
                xray.is_need_restart_core(&hashes)
            }
        }
    };

    if !should_restart {
        tracing::info!("Xray Core configuration is up-to-date - no restart required");
        let system = crate::system_stats::collect_system_snapshot();
        return Json(serde_json::json!({
            "response": {
                "isStarted": true,
                "version": xray_version(&state.env),
                "error": null,
                "nodeInformation": {
                    "version": env!("CARGO_PKG_VERSION")
                },
                "system": system
            }
        }));
    }

    // Persist panel state so we can recover after a Pterodactyl restart.
    let persisted = remnanode_config::persistence::PersistedNodeState {
        panel_config: xray_config.clone(),
        torrent_blocker_state: body.get("torrentBlockerState").cloned().unwrap_or(serde_json::json!({})),
        hashes: hashes.clone(),
        saved_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = persisted.save().await {
        tracing::error!("Failed to persist node state: {e}");
    }

    {
        let mut xray = state.xray.write().await;
        xray.extract_users_from_config(&hashes, &full_config);
        xray.config = Some(full_config);
    }

    if let Err(e) = remnanode_xray::XrayState::start_and_connect(
        state.xray.clone(),
        &state.internal.socket_path,
        &state.internal.token,
        state.env.xtls_api_port,
        state.mtls_certs.clone(),
    ).await {
        return Json(serde_json::json!({
            "response": { "message": e }
        }));
    }

    tracing::info!("Xray started successfully");

    let system = crate::system_stats::collect_system_snapshot();

    Json(serde_json::json!({
        "response": {
            "isStarted": true,
            "version": xray_version(&state.env),
            "error": null,
            "nodeInformation": {
                "version": env!("CARGO_PKG_VERSION")
            },
            "system": system
        }
    }))
}

pub async fn stop(State(state): State<AppState>) -> Json<serde_json::Value> {
    tracing::info!("GET /node/xray/stop");

    // Clear persisted state to match upstream behaviour: a manual stop means
    // the node should not auto-start xray on next boot.
    if let Err(e) = remnanode_config::persistence::PersistedNodeState::clear().await {
        tracing::warn!("Failed to clear persisted state: {e}");
    }

    let mut xray = state.xray.write().await;
    xray.stop_xray().await;
    xray.config = None;
    xray.xtls_config_inbounds.clear();
    xray.inbound_users.clear();
    xray.empty_config_hash = None;
    xray.inbound_hashes.clear();

    Json(serde_json::json!({"response": {"isStopped": true}}))
}

/// Attempt to start xray from a previously persisted panel config.
/// Called once at startup so the node is self-healing across Pterodactyl restarts.
pub async fn auto_start_from_persistence(state: AppState) -> Result<(), String> {
    let persisted = match remnanode_config::persistence::PersistedNodeState::load().await {
        Ok(Some(s)) => s,
        Ok(None) => {
            tracing::info!("No persisted node state found; waiting for panel to start xray");
            return Ok(());
        }
        Err(e) => {
            tracing::warn!("Failed to load persisted state: {e}");
            return Err(e);
        }
    };

    tracing::info!("Auto-starting xray from persisted state (saved at {})", persisted.saved_at);

    ensure_hysteria_cert(&state.env, &persisted.panel_config).await;

    let is_torrent_blocker_enabled = persisted
        .torrent_blocker_state
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let torrent_include_tags: std::collections::HashSet<String> = persisted
        .torrent_blocker_state
        .get("includeRuleTags")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let full_config = remnanode_config::xray_config::generate_api_config(
        &persisted.panel_config,
        state.env.xtls_api_port,
        state.env.xray_proxy_port,
        state.env.node_port,
        &state.mtls_certs,
        is_torrent_blocker_enabled,
        &torrent_include_tags,
        &state.internal.socket_path,
        &state.internal.token,
    );

    {
        let mut xray = state.xray.write().await;
        xray.extract_users_from_config(&persisted.hashes, &full_config);
        xray.config = Some(full_config);
    }

    remnanode_xray::XrayState::start_and_connect(
        state.xray.clone(),
        &state.internal.socket_path,
        &state.internal.token,
        state.env.xtls_api_port,
        state.mtls_certs.clone(),
    ).await
}

/// Daily background check for Hysteria certificate renewal. acme.sh itself
/// decides whether a given cert is actually due (~30 days before the
/// ~90-day Let's Encrypt expiry) — this just calls it once a day and, if the
/// certificate file was actually rewritten, restarts xray so it picks up the
/// new file (xray does not hot-reload TLS certificates from disk).
pub fn spawn_hysteria_cert_renewal(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 3600));
        interval.tick().await; // first tick fires immediately; skip it

        loop {
            interval.tick().await;

            let Some(cf_token) = state.env.cf_token.clone() else {
                continue;
            };

            let persisted = match remnanode_config::persistence::PersistedNodeState::load().await {
                Ok(Some(s)) => s,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!("Cert renewal: failed to load persisted state: {e}");
                    continue;
                }
            };

            if remnanode_config::acme::find_hysteria_domain(&persisted.panel_config).is_none() {
                continue;
            }

            match remnanode_config::acme::renew_if_due(&cf_token).await {
                Ok(true) => {
                    tracing::info!("Hysteria certificate renewed; restarting xray to pick it up");
                    if let Err(e) = auto_start_from_persistence(state.clone()).await {
                        tracing::error!("Failed to restart xray after cert renewal: {e}");
                    }
                }
                Ok(false) => {}
                Err(e) => tracing::warn!("Hysteria certificate renewal check failed: {e}"),
            }
        }
    })
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
            "xrayVersion": xray_version(&state.env),
            "nodeVersion": env!("CARGO_PKG_VERSION")
        }
    }))
}
