pub mod acme;
pub mod certs;
pub mod persistence;
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
    /// Cloudflare API token (DNS:Edit scope) used by acme.sh's `dns_cf`
    /// plugin to complete DNS-01 challenges for Hysteria2's Let's Encrypt
    /// certificate. Only required if the panel config contains a Hysteria
    /// inbound.
    pub cf_token: Option<String>,
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

        let cf_token = std::env::var("CF_TOKEN").ok();

        Ok(Self {
            node_port,
            secret_key,
            api_domain,
            xtls_api_port,
            xray_proxy_port,
            xray_core_version,
            disable_hashed_set_check,
            cf_token,
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

/// Detect whether the current process has CAP_NET_ADMIN in its effective
/// capability set. This mirrors the Node.js `sockdestroy` check used by the
/// upstream Remnawave node.
///
/// Currently unused: the stats path no longer depends on `CAP_NET_ADMIN`
/// (online detection is traffic-recency based via the accumulator, and
/// `statsUserOnline` is forced off). Retained for future privilege-gated
/// features such as nftables-based IP blocking.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub fn has_cap_net_admin() -> bool {
    const CAP_NET_ADMIN: u64 = 12;

    let contents = match std::fs::read_to_string("/proc/self/status") {
        Ok(c) => c,
        Err(_) => return false,
    };

    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("CapEff:\t") {
            return parse_cap_mask(value).map_or(false, |mask| (mask >> CAP_NET_ADMIN) & 1 == 1);
        }
    }

    false
}

#[cfg(target_os = "linux")]
fn parse_cap_mask(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Kernel uses lowercase hex (e.g. 0000003fffffffff); allow uppercase too.
    u64::from_str_radix(trimmed, 16).ok()
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn has_cap_net_admin() -> bool {
    false
}
