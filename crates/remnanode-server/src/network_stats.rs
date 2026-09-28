use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::interval;

#[derive(Debug, Clone)]
pub struct InterfaceRate {
    pub interface: String,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
    pub rx_total: u64,
    pub tx_total: u64,
}

#[derive(Debug, Default)]
struct Snapshot {
    rx_bytes: u64,
    tx_bytes: u64,
    timestamp_ms: u64,
}

#[derive(Debug, Default)]
pub struct NetworkStatsState {
    previous: HashMap<String, Snapshot>,
    current_rates: HashMap<String, InterfaceRate>,
    default_interface: Option<String>,
}

impl NetworkStatsState {
    pub fn default_interface(&self) -> Option<&str> {
        self.default_interface.as_deref()
    }

    pub fn get_default_rate(&self) -> Option<InterfaceRate> {
        self.default_interface
            .as_ref()
            .and_then(|name| self.current_rates.get(name).cloned())
    }
}

pub type SharedNetworkStats = Arc<Mutex<NetworkStatsState>>;

/// Spawn a background task that polls `/proc/net/dev` and `/proc/net/route`
/// once per second, mirroring upstream `NetworkStatsService`.
pub fn spawn_network_stats_polling() -> SharedNetworkStats {
    let state = Arc::new(Mutex::new(NetworkStatsState::default()));
    let state_clone = state.clone();

    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(1));
        // Skip first tick immediately; upstream also does an initial read in onModuleInit.
        {
            let mut guard = state_clone.lock().unwrap();
            guard.default_interface = resolve_default_interface();
            guard.previous = read_proc_net_dev();
        }

        loop {
            ticker.tick().await;
            let mut guard = state_clone.lock().unwrap();
            guard.default_interface = resolve_default_interface();
            let now = read_proc_net_dev();
            let now_ts = current_ms();

            for (iface, current) in &now {
                if let Some(prev) = guard.previous.get(iface) {
                    let elapsed = (now_ts.saturating_sub(prev.timestamp_ms)) as f64 / 1000.0;
                    if elapsed > 0.0 {
                        let rx_rate = (current.rx_bytes.saturating_sub(prev.rx_bytes)) as f64 / elapsed;
                        let tx_rate = (current.tx_bytes.saturating_sub(prev.tx_bytes)) as f64 / elapsed;
                        guard.current_rates.insert(
                            iface.clone(),
                            InterfaceRate {
                                interface: iface.clone(),
                                rx_bytes_per_sec: rx_rate.max(0.0),
                                tx_bytes_per_sec: tx_rate.max(0.0),
                                rx_total: current.rx_bytes,
                                tx_total: current.tx_bytes,
                            },
                        );
                    }
                }
            }

            guard.previous = now;
        }
    });

    state
}

fn read_proc_net_dev() -> HashMap<String, Snapshot> {
    let mut result = HashMap::new();
    let content = match std::fs::read_to_string("/proc/net/dev") {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!("Failed to read /proc/net/dev: {e}");
            return result;
        }
    };

    let timestamp_ms = current_ms();
    for line in content.lines().skip(2) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(iface_with_colon) = parts.next() else { continue };
        let iface = iface_with_colon.trim_end_matches(':');
        let values: Vec<&str> = parts.collect();
        if values.len() < 9 {
            continue;
        }
        let rx_bytes = values[0].parse::<u64>().unwrap_or(0);
        let tx_bytes = values[8].parse::<u64>().unwrap_or(0);
        result.insert(
            iface.to_string(),
            Snapshot {
                rx_bytes,
                tx_bytes,
                timestamp_ms,
            },
        );
    }

    result
}

fn resolve_default_interface() -> Option<String> {
    let content = match std::fs::read_to_string("/proc/net/route") {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!("Failed to read /proc/net/route: {e}");
            return None;
        }
    };

    for line in content.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        if parts[1] == "00000000" {
            return Some(parts[0].to_string());
        }
    }

    None
}

fn current_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
