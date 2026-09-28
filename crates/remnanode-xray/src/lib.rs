pub mod grpc;
pub mod process;

use std::collections::{HashMap, HashSet};
use tokio::process::Child;
use tokio::sync::RwLock;
use std::sync::Arc;

pub struct XrayState {
    pub process: Option<Child>,
    pub config: Option<serde_json::Value>,
    pub xtls_config_inbounds: HashSet<String>,
    pub inbound_users: HashMap<String, HashSet<String>>,
    pub handler_client: Option<grpc::handler::HandlerClient>,
    pub stats_client: Option<grpc::stats::StatsClient>,
    pub router_client: Option<grpc::router::RouterClient>,
    pub mtls_certs: Option<Arc<remnanode_config::MtlsCerts>>,
    pub xtls_api_port: u16,
    /// Hash of the empty base config, used to detect base config changes.
    pub empty_config_hash: Option<String>,
    /// Per-inbound hash of the user set, used to detect user changes.
    pub inbound_hashes: HashMap<String, String>,
}

impl Default for XrayState {
    fn default() -> Self {
        Self {
            process: None,
            config: None,
            xtls_config_inbounds: HashSet::new(),
            inbound_users: HashMap::new(),
            handler_client: None,
            stats_client: None,
            router_client: None,
            mtls_certs: None,
            xtls_api_port: 61000,
            empty_config_hash: None,
            inbound_hashes: HashMap::new(),
        }
    }
}

impl XrayState {
    pub async fn connect_grpc(&mut self, xtls_api_port: u16, mtls_certs: &remnanode_config::MtlsCerts) -> Result<(), String> {
        let addr = format!("127.0.0.1:{xtls_api_port}");
        let ca = mtls_certs.ca_cert_pem.as_bytes();
        let cert = mtls_certs.client_cert_pem.as_bytes();
        let key = mtls_certs.client_key_pem.as_bytes();

        self.handler_client = Some(
            grpc::handler::HandlerClient::connect(&addr, ca, cert, key)
                .await
                .map_err(|e| format!("Handler gRPC connect failed: {e}"))?
        );
        self.stats_client = Some(
            grpc::stats::StatsClient::connect(&addr, ca, cert, key)
                .await
                .map_err(|e| format!("Stats gRPC connect failed: {e}"))?
        );
        self.router_client = Some(
            grpc::router::RouterClient::connect(&addr, ca, cert, key)
                .await
                .map_err(|e| format!("Router gRPC connect failed: {e}"))?
        );

        self.xtls_api_port = xtls_api_port;
        self.mtls_certs = Some(Arc::new(mtls_certs.clone()));
        Ok(())
    }

    /// Stop any running xray process and clear gRPC clients.
    pub async fn stop_xray(&mut self) {
        if let Some(ref mut child) = self.process {
            let _ = crate::process::stop_xray(child).await;
        }
        self.process = None;
        self.handler_client = None;
        self.stats_client = None;
        self.router_client = None;
    }

    /// Start the xray process and connect gRPC clients with retries.
    ///
    /// The lock on `xray` is acquired and released per attempt so that the
    /// internal config server can serve xray's initial config fetch while we
    /// wait for the gRPC API to come up.
    pub async fn start_and_connect(
        xray: Arc<RwLock<XrayState>>,
        socket_path: &str,
        token: &str,
        xtls_api_port: u16,
        mtls_certs: Arc<remnanode_config::MtlsCerts>,
    ) -> Result<(), String> {
        use std::time::Duration;

        // Stop any existing process and clear stale clients.
        {
            let mut guard = xray.write().await;
            guard.stop_xray().await;
        }

        // Spawn xray. It will immediately fetch its config from the internal server.
        let child = match crate::process::start_xray(socket_path, token).await {
            Ok(child) => {
                tracing::info!("Xray process started (PID {:?})", child.id());
                child
            }
            Err(e) => {
                tracing::error!("Failed to start xray: {e}");
                return Err(e);
            }
        };

        {
            let mut guard = xray.write().await;
            guard.process = Some(child);
        }

        // gRPC retry loop. Lock is released between attempts.
        for attempt in 0..20 {
            tokio::time::sleep(Duration::from_secs(2)).await;

            let mut guard = xray.write().await;

            // Check if process is still alive.
            if let Some(ref mut child) = guard.process {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        tracing::error!("Xray process exited prematurely: {status}");
                        guard.process = None;
                        return Err(format!("Xray exited: {status}"));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!("Failed to check xray status: {e}");
                    }
                }
            } else {
                return Err("Xray process lost".to_string());
            }

            match guard.connect_grpc(xtls_api_port, &mtls_certs).await {
                Ok(_) => {
                    // Warm up the stats channel with a real call. The first gRPC
                    // request on a freshly-negotiated HTTP/2 connection can fail
                    // with a transport error on some Xray-core versions; doing it
                    // here while we still hold the startup lock prevents that
                    // failure from reaching the panel's health checks.
                    let warmup_ok = match guard.stats_client.as_mut() {
                        Some(client) => client.get_sys_stats().await.is_ok(),
                        None => false,
                    };

                    if warmup_ok {
                        tracing::info!("gRPC connected and warmed up after {} attempts", attempt + 1);
                        return Ok(());
                    }

                    tracing::warn!("gRPC connection attempt {}/20 succeeded but warmup call failed", attempt + 1);
                    guard.handler_client = None;
                    guard.stats_client = None;
                    guard.router_client = None;
                    if attempt == 19 {
                        return Err("gRPC connection succeeded but warmup call failed".to_string());
                    }
                }
                Err(e) => {
                    tracing::warn!("gRPC attempt {}/20 failed: {e}", attempt + 1);
                    if attempt == 19 {
                        return Err(format!("gRPC connection failed: {e}"));
                    }
                }
            }
            // lock released here at end of scope
        }

        Err("gRPC connection failed after 20 attempts".to_string())
    }

    pub fn add_xtls_config_inbound(&mut self, tag: String) {
        self.xtls_config_inbounds.insert(tag);
    }

    pub fn add_user_to_inbound(&mut self, tag: &str, uuid: &str) {
        self.inbound_users
            .entry(tag.to_string())
            .or_default()
            .insert(uuid.to_string());
    }

    pub fn remove_user_from_inbound(&mut self, tag: &str, uuid: &str) {
        if let Some(users) = self.inbound_users.get_mut(tag) {
            users.remove(uuid);
        }
    }

    /// Extract inbound tags, user UUIDs and inbound hashes from the generated xray config.
    /// Only inbounds whose tag is present in `hashes.inbounds[].tag` are processed,
    /// matching upstream behaviour.
    pub fn extract_users_from_config(&mut self, hashes: &serde_json::Value, config: &serde_json::Value) {
        self.xtls_config_inbounds.clear();
        self.inbound_users.clear();
        self.empty_config_hash = hashes
            .get("emptyConfig")
            .and_then(|v| v.as_str())
            .map(String::from);
        self.inbound_hashes.clear();

        let valid_inbounds: HashMap<String, String> = hashes
            .get("inbounds")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let tag = item.get("tag").and_then(|v| v.as_str())?;
                        let hash = item.get("hash").and_then(|v| v.as_str())?;
                        Some((tag.to_string(), hash.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        if let Some(inbounds) = config.get("inbounds").and_then(|v| v.as_array()) {
            for inbound in inbounds {
                let Some(tag) = inbound.get("tag").and_then(|v| v.as_str()) else {
                    continue;
                };
                if tag == "REMNAWAVE_API_INBOUND" {
                    continue;
                }
                let Some(expected_hash) = valid_inbounds.get(tag) else {
                    continue;
                };

                self.add_xtls_config_inbound(tag.to_string());
                self.inbound_hashes.insert(tag.to_string(), expected_hash.clone());

                let mut count = 0;
                if let Some(clients) = inbound
                    .get("settings")
                    .and_then(|v| v.get("clients"))
                    .and_then(|v| v.as_array())
                {
                    for client in clients {
                        if let Some(id) = client.get("id").and_then(|v| v.as_str()) {
                            self.add_user_to_inbound(tag, id);
                            count += 1;
                        }
                    }
                }

                tracing::info!("{tag} has {count} users");
            }
        }
    }

    /// Compare incoming hashes with the currently stored ones to decide whether
    /// xray needs a full restart. Mirrors upstream `InternalService.isNeedRestartCore`.
    pub fn is_need_restart_core(&self, incoming_hashes: &serde_json::Value) -> bool {
        let Some(current_empty) = self.empty_config_hash.as_ref() else {
            return true;
        };

        let incoming_empty = incoming_hashes
            .get("emptyConfig")
            .and_then(|v| v.as_str());
        if incoming_empty != Some(current_empty.as_str()) {
            tracing::warn!("Detected changes in Xray Core base configuration");
            return true;
        }

        let incoming_inbounds: HashMap<String, String> = incoming_hashes
            .get("inbounds")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let tag = item.get("tag").and_then(|v| v.as_str())?;
                        let hash = item.get("hash").and_then(|v| v.as_str())?;
                        Some((tag.to_string(), hash.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        if incoming_inbounds.len() != self.inbound_hashes.len() {
            tracing::warn!("Number of Xray Core inbounds has changed");
            return true;
        }

        for (tag, stored_hash) in &self.inbound_hashes {
            let Some(incoming_hash) = incoming_inbounds.get(tag) else {
                tracing::warn!("Inbound {tag} no longer exists in Xray Core configuration");
                return true;
            };
            if incoming_hash != stored_hash {
                tracing::warn!("User configuration changed for inbound {tag}");
                return true;
            }
        }

        tracing::info!("Xray Core configuration is up-to-date - no restart required");
        false
    }
}

pub async fn ensure_xray_binary() -> Result<(), String> {
    let xray_path = "/home/container/runtime/bin/rw-core";
    if std::path::Path::new(xray_path).exists() {
        tracing::info!("Xray binary already exists");
        return Ok(());
    }

    let bin_dir = "/home/container/runtime/bin";
    tokio::fs::create_dir_all(bin_dir)
        .await
        .map_err(|e| format!("Failed to create bin dir: {e}"))?;

    let arch = if cfg!(target_arch = "x86_64") {
        "Xray-linux-64"
    } else if cfg!(target_arch = "aarch64") {
        "Xray-linux-arm64-v8a"
    } else {
        "Xray-linux-64"
    };

    let version = std::env::var("XRAY_CORE_VERSION").unwrap_or_else(|_| "v26.3.27".to_string());
    let url = format!(
        "https://github.com/XTLS/Xray-core/releases/download/{version}/{arch}.zip"
    );

    tracing::info!("Downloading xray from {url}");

    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("Download failed with status: {}", response.status()));
    }

    let bytes = response.bytes().await.map_err(|e| format!("Read failed: {e}"))?;

    use std::io::Cursor;
    let reader = Cursor::new(&bytes[..]);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("Zip parse failed: {e}"))?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).unwrap();
        let name = file.name().to_string();
        if name == "xray" || name.ends_with("/xray") {
            let out_path = format!("{bin_dir}/xray");
            let mut out = std::fs::File::create(&out_path)
                .map_err(|e| format!("Create file failed: {e}"))?;
            std::io::copy(&mut file, &mut out)
                .map_err(|e| format!("Write failed: {e}"))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755))
                    .map_err(|e| format!("chmod failed: {e}"))?;
            }
        }
    }

    // Create symlink rw-core -> xray
    let rw_core = format!("{bin_dir}/rw-core");
    let xray = format!("{bin_dir}/xray");
    let _ = std::fs::remove_file(&rw_core);
    std::os::unix::fs::symlink(&xray, &rw_core)
        .map_err(|e| format!("Symlink failed: {e}"))?;

    tracing::info!("Xray binary downloaded and installed");
    Ok(())
}
