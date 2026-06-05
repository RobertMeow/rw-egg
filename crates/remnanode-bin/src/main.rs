use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    tracing::info!("remnanode-rs starting");

    let env = match remnanode_config::EnvConfig::from_env() {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("Failed to parse environment: {e}");
            std::process::exit(1);
        }
    };

    tracing::info!(
        "Configured: port={}, api_domain={}, panel_ips={:?}",
        env.node_port,
        env.api_domain.as_deref().unwrap_or("(none)"),
        env.panel_ips,
    );

    // Generate random internal values
    let internal = remnanode_config::generate_internal_config();

    // Parse SECRET_KEY and generate internal mTLS certs
    let secret = match remnanode_config::parse_secret_key(&env.secret_key) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to parse SECRET_KEY: {e}");
            std::process::exit(1);
        }
    };

    let mtls_certs = remnanode_config::generate_mtls_certs();

    // Download xray binary if not present
    if let Err(e) = remnanode_xray::ensure_xray_binary().await {
        tracing::error!("Failed to prepare xray binary: {e}");
        std::process::exit(1);
    }

    // Create shared app state
    let state = remnanode_server::AppState::new(
        env.clone(),
        internal.clone(),
        secret,
        mtls_certs,
    );

    // Build the axum router
    let app = remnanode_server::build_router(state.clone());

    // Start internal server on Unix socket (for xray config fetching)
    let internal_socket = internal.socket_path.clone();
    let internal_app = app.clone();
    tokio::spawn(async move {
        remnanode_mux::servers::run_internal_server(internal_app, internal_socket).await;
    });

    // Start TLS API server on a local port (for panel communication)
    let api_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind API listener");
    let api_port = api_listener.local_addr().unwrap().port();
    let api_app = app.clone();
    let secret_for_api = state.secret.as_ref().clone();
    tokio::spawn(async move {
        remnanode_mux::servers::run_tls_api_server(api_listener, api_app, &secret_for_api).await;
    });

    // Start the IP-based multiplexer on the public port
    if let Err(e) = remnanode_mux::run_multiplexer(
        env.node_port,
        env.panel_ips,
        env.xray_proxy_port,
        api_port,
        state,
        app,
    )
    .await
    {
        tracing::error!("Multiplexer failed: {e}");
        std::process::exit(1);
    }
}
