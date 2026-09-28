use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Directory where runtime state is persisted across Pterodactyl restarts.
pub fn state_dir() -> PathBuf {
    PathBuf::from("/home/container/runtime/state")
}

/// Path to the persisted node state file.
pub fn state_path() -> PathBuf {
    state_dir().join("node-state.json")
}

/// Persisted panel state so the node can restart xray without waiting for the panel
/// after a Pterodactyl reboot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedNodeState {
    /// Raw xrayConfig object received from the panel.
    pub panel_config: serde_json::Value,
    /// Torrent blocker state received from the panel.
    pub torrent_blocker_state: serde_json::Value,
    /// Hashes payload used to detect configuration changes.
    pub hashes: serde_json::Value,
    /// ISO8601 timestamp of when the state was saved.
    pub saved_at: String,
}

impl PersistedNodeState {
    /// Load persisted state from disk, if present.
    pub async fn load() -> Result<Option<Self>, String> {
        let path = state_path();
        if !path.exists() {
            return Ok(None);
        }

        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| format!("Failed to read persisted state: {e}"))?;

        let state = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Failed to parse persisted state: {e}"))?;

        Ok(Some(state))
    }

    /// Save state to disk, creating parent directories as needed.
    pub async fn save(&self) -> Result<(), String> {
        let dir = state_dir();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("Failed to create state dir: {e}"))?;

        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| format!("Failed to serialize state: {e}"))?;

        tokio::fs::write(state_path(), bytes)
            .await
            .map_err(|e| format!("Failed to write persisted state: {e}"))?;

        Ok(())
    }

    /// Remove persisted state from disk.
    pub async fn clear() -> Result<(), String> {
        let path = state_path();
        if path.exists() {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| format!("Failed to remove persisted state: {e}"))?;
        }
        Ok(())
    }
}
