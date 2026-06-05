use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub struct PluginState {
    pub torrent_blocker: TorrentBlockerState,
    pub config_hash: Option<String>,
    pub blocked_ips: HashSet<String>,
}

#[derive(Debug, Default)]
pub struct TorrentBlockerState {
    pub enabled: bool,
    pub include_rule_tags: HashSet<String>,
    pub reports: Vec<TorrentBlockerReport>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TorrentBlockerReport {
    pub ip: String,
    pub blocked_at: i64,
    pub rule_tag: Option<String>,
}

impl PluginState {
    pub fn sync(&mut self, config: &serde_json::Value) {
        if let Some(tb) = config.get("torrentBlocker") {
            self.torrent_blocker.enabled = tb.get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            self.torrent_blocker.include_rule_tags = tb.get("includeRuleTags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
        }
    }

    pub fn collect_torrent_reports(&mut self) -> Vec<TorrentBlockerReport> {
        std::mem::take(&mut self.torrent_blocker.reports)
    }
}
