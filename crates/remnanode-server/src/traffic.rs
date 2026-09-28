//! Node-side traffic accumulator — the source of truth for per-user and total
//! traffic reported to the panel.
//!
//! In a non-root Pterodactyl container we cannot rely on xray's
//! `statsUserOnline` (it needs `CAP_NET_ADMIN` for kernel connection tracking),
//! and the panel's `reset:true` polling model loses any traffic that flows
//! between two panel polls if xray restarts. This module fixes both:
//!
//! - A background task polls xray's per-user traffic counters with `reset:true`
//!   every few seconds and accumulates them into a persisted map. The node
//!   becomes the sole consumer of those counters, so the panel's
//!   `get-users-stats` is served from here as a delta since the last panel call.
//!   Because the accumulator is persisted to disk, traffic is no longer lost
//!   across xray or container restarts.
//! - Online status is derived from traffic recency: a user is "online" if they
//!   transferred data within the last [`ONLINE_WINDOW_SECS`]. No kernel
//!   privileges required.
//!
//! The [`SharedTraffic`] handle mirrors [`crate::network_stats::SharedNetworkStats`]:
//! an `Arc<std::sync::Mutex<…>>` whose critical sections never span an `.await`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::time::interval;

use crate::state::AppState;
use remnanode_xray::grpc::stats::StatsClient;

/// How often the accumulator polls xray's per-user traffic counters.
const POLL_INTERVAL_SECS: u64 = 10;
/// A user is considered "online" if they generated traffic within this window.
const ONLINE_WINDOW_SECS: u64 = 60;
/// Persist accumulator state to disk at least every N polls.
const PERSIST_EVERY_POLLS: u32 = 3;
/// Drop users not seen for this long from the accumulator, keeping the map
/// (and the persisted file) bounded across restarts.
const PRUNE_AFTER_SECS: u64 = 6 * 3600;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct UserTraffic {
    /// Cumulative uplink bytes transferred by this user since the accumulator
    /// started (monotonically grows as the background poll adds deltas).
    uplink: u64,
    /// Cumulative downlink bytes.
    downlink: u64,
    /// Wall-clock millis of the last poll in which this user had non-zero traffic.
    last_seen_ms: u64,
    /// Cumulative watermark at the time of the panel's last `reset:true` read.
    /// The difference `uplink - last_served_uplink` is the delta returned to the
    /// panel, which matches the xray `reset:true` semantics the panel expects.
    last_served_uplink: u64,
    last_served_downlink: u64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TrafficState {
    users: HashMap<String, UserTraffic>,
    total_uplink: u64,
    total_downlink: u64,
}

pub type SharedTraffic = Arc<Mutex<TrafficState>>;

fn traffic_state_path() -> std::path::PathBuf {
    remnanode_config::persistence::state_dir().join("traffic-state.json")
}

impl TrafficState {
    /// Load persisted accumulator state, or start fresh if absent/unreadable.
    async fn load() -> Self {
        let path = traffic_state_path();
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(_) => return Self::default(),
        };
        match serde_json::from_slice::<TrafficState>(&bytes) {
            Ok(s) => {
                tracing::info!(
                    "Loaded traffic state from {} ({} users, up={}, down={})",
                    path.display(),
                    s.users.len(),
                    s.total_uplink,
                    s.total_downlink
                );
                s
            }
            Err(e) => {
                tracing::warn!("Failed to parse traffic state ({e}); starting fresh");
                Self::default()
            }
        }
    }

    /// Persist already-serialized bytes to disk (best-effort). Callers must
    /// serialize *before* this so the mutex is never held across the await.
    async fn write_bytes(bytes: Vec<u8>) {
        let dir = remnanode_config::persistence::state_dir();
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            tracing::warn!("Failed to create state dir: {e}");
            return;
        }
        if let Err(e) = tokio::fs::write(traffic_state_path(), bytes).await {
            tracing::warn!("Failed to write traffic state: {e}");
        }
    }

    /// Drop users not seen within `PRUNE_AFTER_SECS`. The panel polls far more
    /// often than this, so any unserved delta for a long-inactive user has long
    /// since been served — pruning is safe and keeps the map bounded.
    fn prune_stale(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(PRUNE_AFTER_SECS * 1000);
        self.users.retain(|_, u| u.last_seen_ms == 0 || u.last_seen_ms >= cutoff);
    }

    /// Merge a batch of per-user traffic deltas (as returned by xray's
    /// `reset:true` poll) into the cumulative per-user totals and node-wide
    /// totals, refreshing `last_seen` for users with non-zero traffic.
    fn apply_deltas(&mut self, deltas: Vec<(String, i64, i64)>, now_ms: u64) {
        for (email, up, down) in deltas {
            let up = up.max(0) as u64;
            let down = down.max(0) as u64;
            if up == 0 && down == 0 {
                continue;
            }
            let u = self.users.entry(email).or_default();
            u.uplink = u.uplink.saturating_add(up);
            u.downlink = u.downlink.saturating_add(down);
            u.last_seen_ms = now_ms;
            self.total_uplink = self.total_uplink.saturating_add(up);
            self.total_downlink = self.total_downlink.saturating_add(down);
        }
    }
}

/// Spawn the background accumulator task and return the shared handle.
///
/// The task owns its own [`StatsClient`] (an independent HTTP/2 connection to
/// xray's plaintext localhost API inbound) so it never contends the `state.xray`
/// lock with request handlers. It self-heals across xray restarts: on any gRPC
/// error it drops the client and reconnects on the next tick.
pub fn spawn_traffic_accumulator(state: AppState) -> SharedTraffic {
    let shared: SharedTraffic = Arc::new(Mutex::new(TrafficState::default()));
    let task_shared = shared.clone();

    tokio::spawn(async move {
        // Reload persisted state before the first poll so totals/watermarks
        // survive container restarts.
        let persisted = TrafficState::load().await;
        {
            let mut guard = lock(&task_shared);
            *guard = persisted;
        }

        let addr = format!("127.0.0.1:{}", state.env.xtls_api_port);
        let mtls = state.mtls_certs.clone();
        // StatsClient::connect ignores these (the API inbound is plaintext), but
        // we pass them for signature parity with the handler-side client.
        let ca = mtls.ca_cert_pem.as_bytes().to_vec();
        let cert = mtls.client_cert_pem.as_bytes().to_vec();
        let key = mtls.client_key_pem.as_bytes().to_vec();

        let mut client: Option<StatsClient> = None;
        let mut ticker = interval(Duration::from_secs(POLL_INTERVAL_SECS));
        let mut poll_count: u32 = 0;

        loop {
            ticker.tick().await;

            if client.is_none() {
                match StatsClient::connect(&addr, &ca, &cert, &key).await {
                    Ok(c) => {
                        tracing::info!("Traffic accumulator connected to xray API at {addr}");
                        client = Some(c);
                    }
                    Err(e) => {
                        // xray not up yet (e.g. before auto-start) — retry next tick.
                        tracing::debug!("Traffic accumulator: xray API not ready ({e})");
                        continue;
                    }
                }
            }

            let deltas = match client.as_mut() {
                Some(c) => match c.get_all_users_stats(true).await {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!("Traffic poll failed, will reconnect: {e}");
                        client = None;
                        continue;
                    }
                },
                None => continue,
            };

            let now = now_ms();
            // Serialize compact JSON under the lock (no clone), then write it
            // outside. Persisting on a fixed cadence keeps the clone/serialize
            // churn bounded instead of firing on every panel read.
            let bytes_to_write: Option<Vec<u8>> = {
                let mut guard = lock(&task_shared);
                guard.apply_deltas(deltas, now);
                guard.prune_stale(now);
                poll_count = poll_count.wrapping_add(1);
                if poll_count.is_multiple_of(PERSIST_EVERY_POLLS) {
                    serde_json::to_vec(&*guard).ok()
                } else {
                    None
                }
            };

            if let Some(bytes) = bytes_to_write {
                TrafficState::write_bytes(bytes).await;
            }
        }
    });

    shared
}

/// Per-user traffic for the panel's `get-users-stats` endpoint.
///
/// With `reset = true` (the panel's accounting mode) returns the delta since
/// the last call and advances the per-user watermark (mirroring xray's
/// `reset:true` semantics). With `reset = false` returns cumulative totals.
/// Users with zero traffic in the result are omitted, matching upstream.
pub fn users_delta(shared: &SharedTraffic, reset: bool) -> Vec<(String, u64, u64)> {
    let mut guard = lock(shared);
    let mut out = Vec::new();
    for (email, u) in guard.users.iter_mut() {
        let (up, down) = if reset {
            let du = u.uplink.saturating_sub(u.last_served_uplink);
            let dd = u.downlink.saturating_sub(u.last_served_downlink);
            if du > 0 || dd > 0 {
                u.last_served_uplink = u.uplink;
                u.last_served_downlink = u.downlink;
            }
            (du, dd)
        } else {
            (u.uplink, u.downlink)
        };
        if up > 0 || down > 0 {
            out.push((email.clone(), up, down));
        }
    }
    out
}

/// Whether `username` (the xray client email) is online, by traffic recency.
pub fn is_online(shared: &SharedTraffic, username: &str) -> bool {
    let guard = lock(shared);
    match guard.users.get(username) {
        Some(u) if u.last_seen_ms != 0 => {
            now_ms().saturating_sub(u.last_seen_ms) < ONLINE_WINDOW_SECS * 1000
        }
        _ => false,
    }
}

/// Node-wide cumulative (uplink, downlink) totals.
pub fn totals(shared: &SharedTraffic) -> (u64, u64) {
    let guard = lock(shared);
    (guard.total_uplink, guard.total_downlink)
}

fn lock(shared: &SharedTraffic) -> std::sync::MutexGuard<'_, TrafficState> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> SharedTraffic {
        Arc::new(Mutex::new(TrafficState::default()))
    }

    fn as_map(deltas: Vec<(String, u64, u64)>) -> HashMap<String, (u64, u64)> {
        deltas.into_iter().map(|(e, up, down)| (e, (up, down))).collect()
    }

    #[test]
    fn reset_returns_delta_then_advances_watermark() {
        let shared = empty();
        {
            let mut g = lock(&shared);
            g.apply_deltas(vec![("a@x".into(), 100, 200), ("b@x".into(), 10, 0)], 1000);
            g.apply_deltas(vec![("a@x".into(), 50, 50)], 2000);
        }

        // First reset: full cumulative (a@x = 150/250, b@x = 10/0).
        let m = as_map(users_delta(&shared, true));
        assert_eq!(m.get("a@x").copied(), Some((150, 250)));
        assert_eq!(m.get("b@x").copied(), Some((10, 0)));

        // Second reset immediately: watermark advanced, no new traffic → empty.
        assert!(users_delta(&shared, true).is_empty());

        // More traffic; reset returns only the delta since the last call.
        {
            let mut g = lock(&shared);
            g.apply_deltas(vec![("a@x".into(), 30, 30)], 3000);
        }
        let m = as_map(users_delta(&shared, true));
        assert_eq!(m.get("a@x").copied(), Some((30, 30)));
    }

    #[test]
    fn non_reset_returns_cumulative_without_advancing() {
        let shared = empty();
        {
            let mut g = lock(&shared);
            g.apply_deltas(vec![("a@x".into(), 100, 200)], 1000);
            g.apply_deltas(vec![("a@x".into(), 50, 50)], 2000);
        }

        // reset=false returns cumulative and must NOT advance the watermark.
        let m = as_map(users_delta(&shared, false));
        assert_eq!(m.get("a@x").copied(), Some((150, 250)));

        // A subsequent reset=true therefore still returns the full cumulative.
        let m = as_map(users_delta(&shared, true));
        assert_eq!(m.get("a@x").copied(), Some((150, 250)));
    }

    #[test]
    fn zero_delta_users_are_omitted() {
        let shared = empty();
        {
            let mut g = lock(&shared);
            g.apply_deltas(vec![("a@x".into(), 0, 0), ("b@x".into(), 5, 5)], 1000);
        }
        let m = as_map(users_delta(&shared, true));
        assert!(!m.contains_key("a@x"));
        assert_eq!(m.get("b@x").copied(), Some((5, 5)));
    }

    #[test]
    fn totals_accumulate() {
        let shared = empty();
        {
            let mut g = lock(&shared);
            g.apply_deltas(vec![("a@x".into(), 100, 200), ("b@x".into(), 10, 5)], 1000);
        }
        assert_eq!(totals(&shared), (110, 205));
    }

    #[test]
    fn is_online_is_recency_based() {
        let shared = empty();
        assert!(!is_online(&shared, "a@x"));
        {
            let mut g = lock(&shared);
            g.apply_deltas(vec![("a@x".into(), 1, 1)], now_ms());
        }
        assert!(is_online(&shared, "a@x"));
    }

    #[test]
    fn prune_drops_long_inactive_users() {
        let shared = empty();
        let now = now_ms();
        {
            let mut g = lock(&shared);
            g.apply_deltas(vec![("a@x".into(), 10, 10)], now);
            // b@x last seen 7h ago (older than the 6h prune cutoff).
            g.apply_deltas(vec![("b@x".into(), 5, 5)], now.saturating_sub(7 * 3600 * 1000));
        }
        {
            let mut g = lock(&shared);
            g.prune_stale(now);
        }
        let m = as_map(users_delta(&shared, false));
        assert!(m.contains_key("a@x"));
        assert!(!m.contains_key("b@x"));
    }
}
