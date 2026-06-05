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
        Ok(())
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

    pub fn extract_users_from_config(&mut self, hashes: &serde_json::Value, config: &serde_json::Value) {
        // Extract inbound tags and user UUIDs from config for tracking
        if let Some(inbounds) = config.get("inbounds").and_then(|v| v.as_array()) {
            for inbound in inbounds {
                if let Some(tag) = inbound.get("tag").and_then(|v| v.as_str()) {
                    if tag != "REMNAWAVE_API_INBOUND" {
                        self.add_xtls_config_inbound(tag.to_string());
                    }
                }
            }
        }
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
