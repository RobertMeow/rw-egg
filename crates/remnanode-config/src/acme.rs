//! Let's Encrypt certificate issuance/renewal for the Hysteria2 inbound, via
//! `acme.sh` (DNS-01, Cloudflare `dns_cf` plugin). The Hysteria2 inbound
//! needs a real TLS certificate (unlike Reality, which needs no external CA),
//! and this container has no privileged ports for HTTP-01/TLS-ALPN-01 — DNS-01
//! is the only challenge type that works from an unprivileged Pterodactyl
//! container.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CertPaths {
    pub cert_file: PathBuf,
    pub key_file: PathBuf,
}

/// `$HOME` for the acme.sh subprocess. acme.sh installs itself relative to
/// `$HOME` (default `$HOME/.acme.sh`) and has no reliable `--home <dir>` CLI
/// flag on its *installer* (that flag belongs to the already-installed
/// `acme.sh` binary, not the `get.acme.sh` bootstrap script — passing it to
/// the bootstrap fails with "Unknown parameter"). Pointing `$HOME` at our
/// runtime state dir is the documented, robust way to relocate the whole
/// install.
fn acme_home() -> PathBuf {
    crate::persistence::state_dir().join("acme")
}

/// Where acme.sh actually installs itself under the overridden `$HOME`
/// (i.e. its `LE_WORKING_DIR`, what the installed binary's own `--home`
/// flag expects).
fn acme_working_dir() -> PathBuf {
    acme_home().join(".acme.sh")
}

fn acme_sh_path() -> PathBuf {
    acme_working_dir().join("acme.sh")
}

fn cert_dir() -> PathBuf {
    crate::persistence::state_dir().join("certs/hysteria")
}

/// Fixed on-disk location for the Hysteria certificate. The panel-supplied
/// inbound template points at paths like `/certs/fullchain.cer`, which don't
/// exist in this container — the generated xray config always overrides
/// `streamSettings.tlsSettings.certificates` to point here instead.
pub fn cert_paths() -> CertPaths {
    CertPaths {
        cert_file: cert_dir().join("fullchain.pem"),
        key_file: cert_dir().join("privkey.pem"),
    }
}

/// Find the domain of the first Hysteria inbound in a panel xray config, if
/// any. The domain drives certificate issuance; there's no separate env var
/// for it since the panel already knows it.
pub fn find_hysteria_domain(panel_config: &serde_json::Value) -> Option<String> {
    let inbounds = panel_config.get("inbounds")?.as_array()?;
    inbounds.iter().find_map(|inbound| {
        if inbound.get("protocol").and_then(|v| v.as_str()) != Some("hysteria") {
            return None;
        }
        let tls = inbound.pointer("/streamSettings/tlsSettings")?;
        if let Some(name) = tls.get("serverName").and_then(|v| v.as_str()) {
            return Some(name.to_string());
        }
        tls.get("serverNames")?
            .as_array()?
            .first()?
            .as_str()
            .map(String::from)
    })
}

async fn run_acme_sh(script: &Path, args: &[&str], cf_token: &str) -> Result<std::process::ExitStatus, String> {
    tokio::process::Command::new("sh")
        .arg(script)
        .args(args)
        .env("HOME", acme_home())
        .env("CF_Token", cf_token)
        .status()
        .await
        .map_err(|e| format!("Failed to run acme.sh: {e}"))
}

async fn file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    tokio::fs::metadata(path).await.ok()?.modified().ok()
}

/// Download and install acme.sh into the persisted runtime state dir, if not
/// already present. Mirrors `remnanode_xray::ensure_xray_binary`'s
/// download-on-first-start pattern.
pub async fn ensure_acme_sh_installed() -> Result<PathBuf, String> {
    let script = acme_sh_path();
    if script.exists() {
        return Ok(script);
    }

    let home = acme_home();
    tokio::fs::create_dir_all(&home)
        .await
        .map_err(|e| format!("Failed to create acme.sh home dir: {e}"))?;

    tracing::info!("acme.sh not found — downloading installer");

    let installer_body = reqwest::get("https://get.acme.sh")
        .await
        .map_err(|e| format!("Failed to download acme.sh installer: {e}"))?
        .text()
        .await
        .map_err(|e| format!("Failed to read acme.sh installer body: {e}"))?;

    let installer_path = home.join("acme-install.sh");
    tokio::fs::write(&installer_path, installer_body)
        .await
        .map_err(|e| format!("Failed to write acme.sh installer: {e}"))?;

    // The get.acme.sh bootstrap script takes bare `key`/`key=value` tokens
    // (no leading dashes — see its documented `sh -s email=...` usage) and
    // prepends `--` itself before forwarding to `./acme.sh --install`.
    // Passing an already-dashed flag like `--force` gets double-prefixed
    // into the nonsensical `----force` and rejected. So: bare `force`, no
    // dashes. `--home`/`--noprofile`/`--nocron` are dropped entirely —
    // overriding `$HOME` is the documented way to relocate the install, and
    // it naturally becomes `LE_WORKING_DIR` (`$HOME/.acme.sh`), matching
    // `acme_working_dir()` above. `force` is required here: acme.sh's
    // installer refuses to proceed without a `crontab` binary (absent in
    // this unprivileged container) unless forced — our own daily renewal
    // task replaces the cron job it would otherwise set up.
    let status = tokio::process::Command::new("sh")
        .arg(&installer_path)
        .arg("force")
        .env("HOME", &home)
        .status()
        .await
        .map_err(|e| format!("Failed to run acme.sh installer: {e}"))?;

    if !status.success() {
        return Err(format!("acme.sh installer exited with {status}"));
    }

    if !script.exists() {
        return Err("acme.sh installer completed but acme.sh binary is missing".to_string());
    }

    tracing::info!("acme.sh installed at {}", script.display());
    Ok(script)
}

/// Issue a certificate for `domain` if one isn't already on disk at the
/// fixed path `cert_paths()` returns. Idempotent — safe to call on every
/// startup.
pub async fn ensure_cert(domain: &str, cf_token: &str) -> Result<CertPaths, String> {
    let paths = cert_paths();

    tokio::fs::create_dir_all(cert_dir())
        .await
        .map_err(|e| format!("Failed to create cert dir: {e}"))?;

    if paths.cert_file.exists() && paths.key_file.exists() {
        tracing::info!("Hysteria certificate for {domain} already present at {}", paths.cert_file.display());
        return Ok(paths);
    }

    let acme_sh = ensure_acme_sh_installed().await?;

    tracing::info!("Issuing Let's Encrypt certificate for {domain} via acme.sh (DNS-01/Cloudflare)");

    let status = run_acme_sh(
        &acme_sh,
        &[
            "--issue",
            // acme.sh defaults to ZeroSSL, which requires registering an
            // account email (EAB credentials) before it'll issue anything.
            // We only want Let's Encrypt, which needs no such registration.
            "--server", "letsencrypt",
            "--dns", "dns_cf",
            "-d", domain,
            "--home", &acme_working_dir().to_string_lossy(),
            "--key-file", &paths.key_file.to_string_lossy(),
            "--fullchain-file", &paths.cert_file.to_string_lossy(),
        ],
        cf_token,
    )
    .await?;

    if !status.success() {
        return Err(format!("acme.sh --issue failed with {status}"));
    }

    tracing::info!("Hysteria certificate issued for {domain}");
    Ok(paths)
}

/// Run acme.sh's daily renewal check. acme.sh tracks each cert's own issue
/// date internally and only actually renews within ~30 days of the ~90-day
/// Let's Encrypt expiry, so this is safe to call frequently. Returns whether
/// the certificate file was actually rewritten, so the caller knows whether
/// xray needs restarting to pick up the new file.
pub async fn renew_if_due(cf_token: &str) -> Result<bool, String> {
    let acme_sh = acme_sh_path();
    if !acme_sh.exists() {
        return Ok(false);
    }

    let paths = cert_paths();
    let before = file_mtime(&paths.cert_file).await;

    let status = run_acme_sh(
        &acme_sh,
        &["--cron", "--home", &acme_working_dir().to_string_lossy()],
        cf_token,
    )
    .await?;

    if !status.success() {
        return Err(format!("acme.sh --cron failed with {status}"));
    }

    let after = file_mtime(&paths.cert_file).await;
    Ok(before != after)
}
