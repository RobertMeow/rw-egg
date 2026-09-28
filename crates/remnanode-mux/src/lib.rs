pub mod proxy;
pub mod servers;
pub mod sni;

use crate::proxy::Multiplexer;

pub async fn run_multiplexer(
    port: u16,
    api_domain: Option<String>,
    xray_proxy_port: u16,
    api_internal_port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mux = Multiplexer::new(port, api_domain, xray_proxy_port, api_internal_port);
    mux.run().await
}
