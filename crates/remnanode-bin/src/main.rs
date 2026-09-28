use tracing_subscriber::EnvFilter;

mod speedtest;

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

    if env.api_domain.is_none() {
        tracing::warn!("API_DOMAIN is not set; SNI-based API routing is disabled");
    }

    tracing::info!(
        "Configured: port={}, api_domain={}",
        env.node_port,
        env.api_domain.as_deref().unwrap_or("(none)"),
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

    // Run a quick speed test before starting services so the logged numbers
    // reflect raw container bandwidth without xray/proxy traffic.
    speedtest::run().await;

    // Create shared app state
    let state = remnanode_server::AppState::new(
        env.clone(),
        internal.clone(),
        secret,
        mtls_certs,
    );

    // Start background network interface stats polling.
    let network_stats = remnanode_server::network_stats::spawn_network_stats_polling();
    let state = state.with_network_stats(network_stats);

    // Start the traffic accumulator (source of truth for per-user/total traffic
    // and online-user detection). It owns its own xray gRPC client and self-heals
    // across xray restarts, so it is safe to spawn before xray auto-starts.
    let traffic = remnanode_server::traffic::spawn_traffic_accumulator(state.clone());
    let state = state.with_traffic(traffic);

    // Build the axum router
    let app = remnanode_server::build_router(state.clone());

    // Start internal server on Unix socket (for xray config fetching).
    // This MUST be running before xray is spawned so it can fetch its config.
    let internal_socket = internal.socket_path.clone();
    let internal_app = app.clone();
    tokio::spawn(async move {
        remnanode_mux::servers::run_internal_server(internal_app, internal_socket).await;
    });

    // Give the internal server a moment to bind before xray tries to connect.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Daily check for Hysteria2 certificate renewal (no-op if no Hysteria
    // inbound / CF_TOKEN is configured). auto_start_from_persistence below
    // already issues the initial certificate before xray's first start.
    remnanode_server::handlers::xray::spawn_hysteria_cert_renewal(state.clone());

    // If we have a persisted config from a previous run, auto-start xray so the
    // node recovers automatically after a Pterodactyl restart.
    if let Err(e) = remnanode_server::handlers::xray::auto_start_from_persistence(state.clone()).await {
        tracing::error!("Failed to auto-start xray from persisted state: {e}");
    }

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

    // Start the SNI-based multiplexer on the public port
    if let Err(e) = remnanode_mux::run_multiplexer(
        env.node_port,
        env.api_domain.clone(),
        env.xray_proxy_port,
        api_port,
    )
    .await
    {
        tracing::error!("Multiplexer failed: {e}");
        std::process::exit(1);
    }
}
