use std::sync::Arc;
use tokio::sync::RwLock;
use remnanode_config::{EnvConfig, InternalConfig, SecretKey, MtlsCerts};

#[derive(Clone)]
pub struct AppState {
    pub env: EnvConfig,
    pub internal: InternalConfig,
    pub secret: Arc<SecretKey>,
    pub mtls_certs: Arc<MtlsCerts>,
    pub xray: Arc<RwLock<remnanode_xray::XrayState>>,
    pub plugins: Arc<RwLock<remnanode_plugins::PluginState>>,
}

impl AppState {
    pub fn new(
        env: EnvConfig,
        internal: InternalConfig,
        secret: SecretKey,
        mtls_certs: MtlsCerts,
    ) -> Self {
        Self {
            env,
            internal,
            secret: Arc::new(secret),
            mtls_certs: Arc::new(mtls_certs),
            xray: Arc::new(RwLock::new(remnanode_xray::XrayState::default())),
            plugins: Arc::new(RwLock::new(remnanode_plugins::PluginState::default())),
        }
    }
}
