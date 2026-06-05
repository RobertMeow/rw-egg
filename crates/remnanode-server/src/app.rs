use axum::{Router, routing::{get, post}, middleware};
use axum::extract::FromRef;
use crate::state::AppState;
use crate::handlers;
use crate::middleware::jwt_auth;

pub fn build_router(state: AppState) -> Router {
    let internal_routes = Router::new()
        .route("/get-config", get(handlers::internal::get_config))
        .route("/webhook", post(handlers::internal::webhook));

    let node_routes = Router::new()
        .route("/xray/start", post(handlers::xray::start))
        .route("/xray/stop", get(handlers::xray::stop))
        .route("/xray/healthcheck", get(handlers::xray::healthcheck))
        .route("/handler/add-user", post(handlers::handler::add_user))
        .route("/handler/remove-user", post(handlers::handler::remove_user))
        .route("/handler/add-users", post(handlers::handler::add_users))
        .route("/handler/remove-users", post(handlers::handler::remove_users))
        .route("/handler/get-inbound-users", post(handlers::handler::get_inbound_users))
        .route("/handler/get-inbound-users-count", post(handlers::handler::get_inbound_users_count))
        .route("/handler/drop-users-connections", post(handlers::handler::drop_users_connections))
        .route("/handler/drop-ips", post(handlers::handler::drop_ips))
        .route("/stats/get-user-online-status", post(handlers::stats::get_user_online_status))
        .route("/stats/get-users-stats", post(handlers::stats::get_users_stats))
        .route("/stats/get-system-stats", get(handlers::stats::get_system_stats))
        .route("/stats/get-inbound-stats", post(handlers::stats::get_inbound_stats))
        .route("/stats/get-outbound-stats", post(handlers::stats::get_outbound_stats))
        .route("/stats/get-all-outbounds-stats", post(handlers::stats::get_all_outbounds_stats))
        .route("/stats/get-all-inbounds-stats", post(handlers::stats::get_all_inbounds_stats))
        .route("/stats/get-combined-stats", post(handlers::stats::get_combined_stats))
        .route("/stats/get-user-ip-list", post(handlers::stats::get_user_ip_list))
        .route("/stats/get-users-ip-list", get(handlers::stats::get_users_ip_list))
        .route("/plugin/sync", post(handlers::plugin::sync))
        .route("/plugin/torrent-blocker/collect", post(handlers::plugin::torrent_blocker_collect))
        .route("/plugin/nftables/block-ips", post(handlers::plugin::nftables_block_ips))
        .route("/plugin/nftables/unblock-ips", post(handlers::plugin::nftables_unblock_ips))
        .route("/plugin/nftables/recreate-tables", post(handlers::plugin::nftables_recreate_tables))
        .layer(middleware::from_fn_with_state(state.clone(), jwt_auth));

    let vision_routes = Router::new()
        .route("/block-ip", post(handlers::vision::block_ip))
        .route("/unblock-ip", post(handlers::vision::unblock_ip))
        .layer(middleware::from_fn_with_state(state.clone(), jwt_auth));

    Router::new()
        .nest("/internal", internal_routes)
        .nest("/node", node_routes)
        .merge(vision_routes)
        .with_state(state)
}
