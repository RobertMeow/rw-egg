pub mod proxy;
pub mod servers;
pub mod sni;

use crate::proxy::Multiplexer;
use remnanode_server::AppState;

pub async fn run_multiplexer(
    port: u16,
    panel_ips: Vec<std::net::IpAddr>,
    xray_proxy_port: u16,
    api_internal_port: u16,
    state: AppState,
    app: axum::Router,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mux = Multiplexer::new(port, panel_ips, xray_proxy_port, api_internal_port, state, app);
    mux.run().await
}
