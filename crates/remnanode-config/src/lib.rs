pub mod certs;
pub mod secret;
pub mod xray_config;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct EnvConfig {
    pub node_port: u16,
    pub secret_key: String,
    pub api_domain: Option<String>,
    pub xtls_api_port: u16,
    pub xray_proxy_port: u16,
    pub xray_core_version: String,
    pub disable_hashed_set_check: bool,
    pub panel_ips: Vec<std::net::IpAddr>,
}

impl EnvConfig {
    pub fn from_env() -> Result<Self, String> {
        let node_port: u16 = std::env::var("NODE_PORT")
            .map_err(|_| "NODE_PORT is required".to_string())?
            .parse()
            .map_err(|e| format!("Invalid NODE_PORT: {e}"))?;

        let secret_key = std::env::var("SECRET_KEY")
            .map_err(|_| "SECRET_KEY is required".to_string())?;

        let api_domain = std::env::var("API_DOMAIN").ok();

        let xtls_api_port: u16 = std::env::var("XTLS_API_PORT")
            .unwrap_or_else(|_| "61000".to_string())
            .parse()
            .map_err(|e| format!("Invalid XTLS_API_PORT: {e}"))?;

        let xray_proxy_port: u16 = std::env::var("XRAY_PROXY_PORT")
            .unwrap_or_else(|_| "61001".to_string())
            .parse()
            .map_err(|e| format!("Invalid XRAY_PROXY_PORT: {e}"))?;

        let xray_core_version = std::env::var("XRAY_CORE_VERSION")
            .unwrap_or_else(|_| "v26.3.27".to_string());

        let disable_hashed_set_check = std::env::var("DISABLE_HASHED_SET_CHECK")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);

        let panel_ips: Vec<std::net::IpAddr> = std::env::var("PANEL_IPS")
            .unwrap_or_else(|_| "".to_string())
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();

        Ok(Self {
            node_port,
            secret_key,
            api_domain,
            xtls_api_port,
            xray_proxy_port,
            xray_core_version,
            disable_hashed_set_check,
            panel_ips,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalConfig {
    pub socket_path: String,
    pub token: String,
    pub supervisord_socket_path: String,
    pub supervisord_pid_path: String,
    pub supervisord_user: String,
    pub supervisord_password: String,
}

pub fn generate_internal_config() -> InternalConfig {
    use rand::Rng;
    let mut rng = rand::rng();
    let rnd: String = (&mut rng)
        .sample_iter(rand::distr::Alphanumeric)
        .take(8)
        .map(char::from)
        .collect();

    let mut random_hex = |len: usize| -> String {
        (&mut rng)
            .sample_iter(rand::distr::Alphanumeric)
            .take(len)
            .map(char::from)
            .collect()
    };

    InternalConfig {
        socket_path: format!("/tmp/remnawave-internal-{rnd}.sock"),
        token: random_hex(64),
        supervisord_socket_path: format!("/tmp/supervisord-{rnd}.sock"),
        supervisord_pid_path: format!("/tmp/supervisord-{rnd}.pid"),
        supervisord_user: random_hex(64),
        supervisord_password: random_hex(64),
    }
}

pub use certs::{generate_mtls_certs, MtlsCerts};
pub use secret::{parse_secret_key, SecretKey};
