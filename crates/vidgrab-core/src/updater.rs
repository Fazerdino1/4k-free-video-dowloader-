//! Automated download, SHA-256 verification, and update of the standalone Linux `yt-dlp` binary.
//!
//! Fetches `yt-dlp_linux` and `SHA2-256SUMS` from the official GitHub releases,
//! verifies the cryptographic digest, and sets `0755` executable permissions.

use crate::config::AppPaths;
use crate::error::{CoreError, CoreResult};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::info;

const YTDLP_RELEASE_BASE_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
const YTDLP_SHA256SUMS_URL: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS";

/// Resolves the official `yt-dlp` release download URL for a specific asset name.
#[must_use]
pub fn ytdlp_asset_url(asset_name: &str) -> String {
    format!("{YTDLP_RELEASE_BASE_URL}/{asset_name}")
}

/// Computes the lower-case hex SHA-256 digest of a byte slice.
pub fn compute_sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Extracts the expected SHA-256 hash for `binary_name` (e.g., `"yt-dlp_linux"`) from
/// the contents of `SHA2-256SUMS`.
pub fn parse_sha256sums(sums_text: &str, binary_name: &str) -> Option<String> {
    for line in sums_text.lines() {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let filename = parts.next()?.trim_start_matches('*');
        if filename == binary_name && hash.len() == 64 {
            return Some(hash.to_lowercase());
        }
    }
    None
}

/// Verifies `data` against `expected_hex`, returning `CoreError::ChecksumMismatch` on mismatch.
pub fn verify_sha256(data: &[u8], expected_hex: &str) -> CoreResult<()> {
    let actual = compute_sha256_hex(data);
    let expected = expected_hex.trim().to_lowercase();
    if actual != expected {
        return Err(CoreError::ChecksumMismatch { expected, actual });
    }
    Ok(())
}

/// Writes an executable binary to `dest_path` with `0755` permissions.
pub fn write_executable_binary(dest_path: &Path, bytes: &[u8]) -> CoreResult<()> {
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = dest_path.with_extension("tmp");
    fs::write(&tmp_path, bytes)?;
    let mut perms = fs::metadata(&tmp_path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&tmp_path, perms)?;
    fs::rename(&tmp_path, dest_path)?;
    Ok(())
}

/// Checks whether a candidate `yt-dlp` executable is runnable and returns its version string.
async fn probe_yt_dlp_version(bin: &Path) -> Option<String> {
    let output = Command::new(bin)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if output.status.success() {
        let ver = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !ver.is_empty() {
            return Some(ver);
        }
    }
    None
}

/// Ensures `yt-dlp` is ready (preferring sandbox/musl/NixOS system binary when applicable,
/// otherwise managing `~/.local/share/vidgrab/bin/yt-dlp` for the current CPU architecture/libc).
pub async fn ensure_or_update_yt_dlp(paths: &AppPaths, force_download: bool) -> CoreResult<String> {
    let is_musl = crate::platform::detect_libc() == "musl";
    let is_nixos = Path::new("/etc/NIXOS").exists();
    let in_sandbox = std::env::var_os("FLATPAK_ID").is_some()
        || std::env::var_os("SNAP").is_some()
        || is_nixos
        || is_musl;

    if !force_download && in_sandbox {
        if let Ok(sys_yt) = which::which("yt-dlp") {
            if let Some(ver) = probe_yt_dlp_version(&sys_yt).await {
                return Ok(ver);
            }
        }
    }

    let target_path = paths.managed_yt_dlp_path();

    if target_path.exists() && !force_download {
        if let Some(ver) = probe_yt_dlp_version(&target_path).await {
            return Ok(ver);
        }
    }

    // If already present and force_download is requested, try `yt-dlp -U` first,
    // and fall back to a fresh verified download.
    if target_path.exists() && force_download {
        info!("Updating existing yt-dlp binary via yt-dlp -U");
        if let Ok(output) = Command::new(&target_path)
            .arg("-U")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
        {
            if output.status.success() {
                let msg = String::from_utf8_lossy(&output.stdout).trim().to_string();
                return Ok(msg);
            }
        }
    }

    let arch = std::env::consts::ARCH;
    let candidates = crate::platform::preferred_ytdlp_asset_names(
        arch,
        crate::platform::detect_libc(),
        is_nixos,
    );
    info!(
        "Streaming official yt-dlp ({arch}/{} -> candidates {:?}) and SHA2-256SUMS to {:?}",
        crate::platform::detect_libc(),
        candidates,
        target_path
    );
    let client = crate::shared_http_client();

    let sums_text = client
        .get(YTDLP_SHA256SUMS_URL)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let (asset_name, expected_hash) = candidates
        .iter()
        .find_map(|name| parse_sha256sums(&sums_text, name).map(|h| (*name, h)))
        .ok_or_else(|| {
            CoreError::YtDlpFailed(format!(
                "Aucun binaire compatible trouvé dans SHA2-256SUMS pour {arch}"
            ))
        })?;

    if asset_name == "yt-dlp" && which::which("python3").is_err() {
        return Err(CoreError::YtDlpFailed(
            "Le binaire portable yt-dlp nécessite python3 (installez python3 ou yt-dlp via votre gestionnaire de paquets)"
                .to_string(),
        ));
    }

    if let Some(parent) = target_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp_path = target_path.with_extension("tmp");
    let mut file = tokio::fs::File::create(&tmp_path).await?;
    let mut hasher = Sha256::new();

    let download_url = ytdlp_asset_url(asset_name);
    let resp = client.get(&download_url).send().await?.error_for_status()?;
    let mut stream = resp.bytes_stream();
    while let Some(chunk_res) = stream.next().await {
        let chunk = match chunk_res {
            Ok(c) => c,
            Err(e) => {
                drop(file);
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Err(e.into());
            }
        };
        hasher.update(&chunk);
        if let Err(e) = file.write_all(&chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(e.into());
        }
    }
    file.flush().await?;
    drop(file);

    let actual_hash = hex::encode(hasher.finalize());
    let expected_norm = expected_hash.trim().to_lowercase();
    if actual_hash != expected_norm {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(CoreError::ChecksumMismatch {
            expected: expected_norm,
            actual: actual_hash,
        });
    }

    let mut perms = fs::metadata(&tmp_path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&tmp_path, perms)?;
    tokio::fs::rename(&tmp_path, &target_path).await?;

    let ver_output = Command::new(&target_path)
        .arg("--version")
        .stdout(Stdio::piped())
        .output()
        .await?;
    let ver = String::from_utf8_lossy(&ver_output.stdout)
        .trim()
        .to_string();
    info!(
        "Installed yt-dlp version {} (streamed + SHA-256 verified)",
        ver
    );
    Ok(ver)
}
