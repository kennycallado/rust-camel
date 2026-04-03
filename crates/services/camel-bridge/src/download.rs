use std::path::{Path, PathBuf};

use crate::process::BridgeError;

/// Download the bridge tarball and unpack it to `cache_dir/{version}/`.
///
/// **Development override:** Set `CAMEL_JMS_BRIDGE_BINARY_PATH` to the absolute
/// path of a locally compiled `jms-bridge` binary to skip download entirely.
/// Useful when iterating on the bridge locally:
///
/// ```bash
/// cd bridges/jms && ./jlink.sh   # builds the binary
/// export CAMEL_JMS_BRIDGE_BINARY_PATH=/path/to/jms-bridge
/// cargo run -p jms-example
/// ```
pub async fn ensure_binary(version: &str, cache_dir: &Path) -> Result<PathBuf, BridgeError> {
    // Development override: use a local binary directly, skip download + verification.
    if let Ok(local_path) = std::env::var("CAMEL_JMS_BRIDGE_BINARY_PATH") {
        let path = PathBuf::from(&local_path);
        if path.is_file() {
            tracing::info!(
                "CAMEL_JMS_BRIDGE_BINARY_PATH set — using local bridge binary: {}",
                path.display()
            );
            return Ok(path);
        }
        return Err(BridgeError::Download(format!(
            "CAMEL_JMS_BRIDGE_BINARY_PATH points to a non-existent file: {local_path}"
        )));
    }

    let bin_path = cache_dir.join(version).join("bin").join("jms-bridge");
    let hash_path = cache_dir.join(version).join(".binary.sha256");
    if bin_path.exists()
        && hash_path.exists()
        && let (Ok(stored_hash), Ok(bin_bytes)) = (
            std::fs::read_to_string(&hash_path),
            std::fs::read(&bin_path),
        )
    {
        let actual_hash = sha256_hex(&bin_bytes);
        if stored_hash.trim() == actual_hash {
            tracing::debug!(
                "jms-bridge binary found in verified cache: {}",
                bin_path.display()
            );
            return Ok(bin_path);
        }
        tracing::warn!("jms-bridge cache checksum mismatch — re-downloading");
    }

    let base_url = release_base_url(version)?;
    let tarball_name = tarball_filename(version)?;

    let checksums = fetch_sha256sums(&base_url, version).await?;

    let tarball_bytes = fetch_bytes(&format!("{base_url}/{tarball_name}")).await?;

    let expected = checksums
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?;
            if name.ends_with(&tarball_name) {
                Some(hash.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            BridgeError::Download(format!("tarball not found in SHA256SUMS: {tarball_name}"))
        })?;

    let actual = sha256_hex(&tarball_bytes);
    let tarball_bytes = if actual != expected {
        // Retry once on checksum mismatch (transient corruption)
        tracing::warn!("tarball checksum mismatch on first download, retrying...");
        let retry_bytes = fetch_bytes(&format!("{base_url}/{tarball_name}")).await?;
        let retry_actual = sha256_hex(&retry_bytes);
        if retry_actual != expected {
            return Err(BridgeError::ChecksumMismatch {
                expected,
                actual: retry_actual,
            });
        }
        retry_bytes
    } else {
        tarball_bytes
    };

    let dest = cache_dir.join(version);
    std::fs::create_dir_all(&dest).map_err(BridgeError::Io)?;
    unpack_tarball(&tarball_bytes, &dest)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&bin_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms)?;
    }

    // Write the binary's SHA256 to cache for future integrity checks
    let bin_bytes = std::fs::read(&bin_path).map_err(BridgeError::Io)?;
    let binary_hash = sha256_hex(&bin_bytes);
    std::fs::write(&hash_path, &binary_hash).map_err(BridgeError::Io)?;

    Ok(bin_path)
}

fn release_base_url(version: &str) -> Result<String, BridgeError> {
    let override_url = std::env::var("CAMEL_JMS_BRIDGE_RELEASE_URL").ok();
    resolve_release_base_url(override_url.as_deref(), version)
}

fn resolve_release_base_url(
    override_url: Option<&str>,
    version: &str,
) -> Result<String, BridgeError> {
    let url = match override_url {
        Some(u) => u.to_string(),
        None => format!(
            "https://github.com/rust-camel/rust-camel/releases/download/jms-bridge-{version}"
        ),
    };
    match url::Url::parse(&url) {
        Ok(parsed) => {
            if parsed.host_str() != Some("github.com") {
                return Err(BridgeError::UrlNotAllowed(url));
            }
        }
        Err(_) => return Err(BridgeError::UrlNotAllowed(url)),
    }
    Ok(url)
}

fn tarball_filename(version: &str) -> Result<String, BridgeError> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        other => {
            return Err(BridgeError::Download(format!(
                "camel-jms bridge is not supported on OS: {other}"
            )));
        }
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => {
            return Err(BridgeError::Download(format!(
                "camel-jms bridge is not supported on arch: {other}"
            )));
        }
    };
    Ok(format!("jms-bridge-{version}-{os}-{arch}.tar.gz"))
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, BridgeError> {
    let bytes = reqwest::get(url)
        .await
        .map_err(|e| BridgeError::Download(e.to_string()))?
        .error_for_status()
        .map_err(|e| BridgeError::Download(e.to_string()))?
        .bytes()
        .await
        .map_err(|e| BridgeError::Download(e.to_string()))?;
    Ok(bytes.to_vec())
}

async fn fetch_sha256sums(base_url: &str, version: &str) -> Result<String, BridgeError> {
    let sums_url = format!("{base_url}/jms-bridge-{version}-SHA256SUMS");
    let bytes = fetch_bytes(&sums_url).await?;
    String::from_utf8(bytes)
        .map_err(|e| BridgeError::Download(format!("SHA256SUMS UTF-8 error: {e}")))
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(data);
    hex::encode(hash)
}

fn unpack_tarball(data: &[u8], dest: &Path) -> Result<(), BridgeError> {
    use flate2::read::GzDecoder;
    use tar::Archive;
    let gz = GzDecoder::new(data);
    let mut archive = Archive::new(gz);
    archive.unpack(dest).map_err(BridgeError::Io)
}

/// Default cache dir: `~/.cache/rust-camel/jms-bridge`
pub fn default_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("rust-camel")
        .join("jms-bridge")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_release_base_url_default_contains_github() {
        let url = resolve_release_base_url(None, "0.1.0").unwrap();
        assert!(url.starts_with("https://github.com/"));
        assert!(url.contains("jms-bridge-0.1.0"));
    }

    #[test]
    fn resolve_release_base_url_override_allowed() {
        let url = resolve_release_base_url(
            Some("https://github.com/myorg/myrepo/releases/download/jms-bridge-0.1.0"),
            "0.1.0",
        )
        .unwrap();
        assert!(url.starts_with("https://github.com/"));
    }

    #[test]
    fn resolve_release_base_url_override_not_allowed() {
        let err = resolve_release_base_url(
            Some("https://evil.com/malware.tar.gz"),
            "0.1.0",
        )
        .unwrap_err();
        assert!(matches!(err, BridgeError::UrlNotAllowed(_)));
    }

    #[test]
    fn resolve_release_base_url_override_subdomain_not_allowed() {
        let err = resolve_release_base_url(
            Some("https://github.com.evil.com/malware.tar.gz"),
            "0.1.0",
        )
        .unwrap_err();
        assert!(matches!(err, BridgeError::UrlNotAllowed(_)));
    }

    #[test]
    fn sha256_hex_is_deterministic() {
        let a = sha256_hex(b"hello");
        let b = sha256_hex(b"hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn tarball_filename_contains_version() {
        let name = tarball_filename("0.1.0").unwrap();
        assert!(name.starts_with("jms-bridge-0.1.0-"));
        assert!(name.ends_with(".tar.gz"));
    }
}
