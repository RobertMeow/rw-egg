use tokio::process::Command;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

const XRAY_PATH: &str = "/home/container/runtime/bin/rw-core";
const LOG_DIR: &str = "/home/container/runtime/logs";

pub async fn start_xray(
    socket_path: &str,
    token: &str,
) -> Result<tokio::process::Child, String> {
    tokio::fs::create_dir_all(LOG_DIR)
        .await
        .map_err(|e| format!("Failed to create log dir: {e}"))?;

    let config_url = format!(
        "http+unix://{socket_path}:/internal/get-config?token={token}"
    );

    tracing::info!("Starting xray with config from {config_url}");

    let mut child = Command::new(XRAY_PATH)
        .arg("-config")
        .arg(&config_url)
        .arg("-format")
        .arg("json")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn xray: {e}"))?;

    // Stream stdout lines to main log
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!("[xray] {line}");
            }
        });
    }

    // Stream stderr lines to main log
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::warn!("[xray stderr] {line}");
            }
        });
    }

    Ok(child)
}

pub async fn stop_xray(child: &mut tokio::process::Child) -> Result<(), String> {
    if let Some(id) = child.id() {
        tracing::info!("Stopping xray (PID {id})");
        unsafe {
            libc::kill(id as i32, libc::SIGTERM);
        }
    }

    match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
        Ok(Ok(status)) => {
            tracing::info!("Xray exited with status: {status}");
            Ok(())
        }
        _ => {
            tracing::warn!("Xray did not exit gracefully, force killing");
            let _ = child.kill().await;
            let _ = child.wait().await;
            Ok(())
        }
    }
}
