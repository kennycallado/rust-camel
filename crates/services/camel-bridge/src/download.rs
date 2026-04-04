use std::path::{Path, PathBuf};

use crate::process::BridgeError;

/// Walk up from `CARGO_MANIFEST_DIR` looking for the workspace root.
/// The root is identified by a `Cargo.toml` containing `[workspace]`
/// AND a `bridges/jms/` directory as sentinel.
///
/// Returns `None` if not in the rust-camel workspace (production installs,
/// external dependencies) — the binary resolution silently falls through to
/// the download cache.
fn find_workspace_root() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    find_workspace_root_from_path(&manifest_dir)
}

pub(crate) fn find_workspace_root_from_path(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    for _ in 0..10 {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists() {
            if let Ok(contents) = std::fs::read_to_string(&cargo_toml) {
                if contents.contains("[workspace]") && current.join("bridges").join("jms").exists()
                {
                    return Some(current);
                }
            }
        }
        if !current.pop() {
            break;
        }
    }
    None
}

/// Locate or download the `jms-bridge` binary.
///
/// Resolution order:
///
/// 1. `CAMEL_JMS_BRIDGE_BINARY_PATH` env var — explicit path override
/// 2. `{workspace_root}/bridges/jms/build/native/jms-bridge` — local build from `cargo xtask build-jms-bridge` (auto-detected, dev only)
/// 3. `{cache_dir}/{version}/bin/jms-bridge` — previously downloaded binary
/// 4. Download from GitHub Releases, verify SHA256, unpack to cache
///
/// **Development (rust-camel workspace):**
/// ```bash
/// cargo xtask build-jms-bridge   # one-time build
/// cargo run -p jms-example       # binary auto-detected, no env vars needed
/// ```
///
/// **Override:** Set `CAMEL_JMS_BRIDGE_BINARY_PATH` to skip all detection.
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

    // Step 2: auto-detect local build from `cargo xtask build-jms-bridge`
    if let Some(workspace_root) = find_workspace_root() {
        let local_path = workspace_root
            .join("bridges")
            .join("jms")
            .join("build")
            .join("native")
            .join("jms-bridge");
        if local_path.is_file() {
            tracing::debug!(
                "Found local xtask build at {} — skipping download",
                local_path.display()
            );
            return Ok(local_path);
        } else {
            tracing::debug!(
                "Local xtask build not found at {} — falling through to cache/download",
                local_path.display()
            );
        }
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
            "https://github.com/kennycallado/rust-camel/releases/download/jms-bridge-v{version}"
        ),
    };
    match url::Url::parse(&url) {
        Ok(parsed) => {
            if parsed.scheme() != "https" {
                return Err(BridgeError::UrlNotAllowed(url));
            }
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
        "macos" => {
            return Err(BridgeError::Download(
                "Pre-built camel-jms bridge binaries are not available for macOS. \
                 Build the bridge manually with `cargo xtask build-jms-bridge` (requires Docker with Rosetta/Linux support) \
                 and set CAMEL_JMS_BRIDGE_BINARY_PATH to the resulting binary path.".to_string()
            ));
        }
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
    for entry in archive.entries().map_err(BridgeError::Io)? {
        let mut entry = entry.map_err(BridgeError::Io)?;
        let path = entry.path().map_err(BridgeError::Io)?;
        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(BridgeError::Download(format!(
                "tarball path traversal attempt: {}",
                path.display()
            )));
        }
        entry.unpack_in(dest).map_err(BridgeError::Io)?;
    }
    Ok(())
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
        assert!(url.contains("jms-bridge-v0.1.0"));
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
        let err =
            resolve_release_base_url(Some("https://evil.com/malware.tar.gz"), "0.1.0").unwrap_err();
        assert!(matches!(err, BridgeError::UrlNotAllowed(_)));
    }

    #[test]
    fn resolve_release_base_url_override_subdomain_not_allowed() {
        let err =
            resolve_release_base_url(Some("https://github.com.evil.com/malware.tar.gz"), "0.1.0")
                .unwrap_err();
        assert!(matches!(err, BridgeError::UrlNotAllowed(_)));
    }

    #[test]
    fn resolve_release_base_url_override_http_not_allowed() {
        let err =
            resolve_release_base_url(Some("http://github.com/rust-camel/rust-camel"), "0.1.0")
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
    #[cfg(target_os = "linux")]
    fn tarball_filename_contains_version() {
        let name = tarball_filename("0.1.0").unwrap();
        assert!(name.starts_with("jms-bridge-0.1.0-"));
        assert!(name.ends_with(".tar.gz"));
    }

    #[test]
    fn find_workspace_root_found_in_this_repo() {
        // CARGO_MANIFEST_DIR is .../crates/services/camel-bridge
        // Walking up should find the workspace root with bridges/jms/
        let root = find_workspace_root();
        assert!(root.is_some(), "expected to find workspace root");
        let root = root.unwrap();
        assert!(
            root.join("bridges").join("jms").exists(),
            "sentinel bridges/jms/ should exist"
        );
    }

    #[test]
    fn find_workspace_root_none_without_sentinel() {
        use std::fs;
        let dir = std::env::temp_dir().join("camel-bridge-test-ws");
        let sub = dir.join("x").join("y");
        fs::create_dir_all(&sub).unwrap();
        fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();

        let result = find_workspace_root_from_path(&sub);
        assert_eq!(result, None);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sha256sums_url_uses_version_prefixed_filename() {
        // The SHA256SUMS file uploaded to GitHub Releases is named
        // "jms-bridge-{version}-SHA256SUMS". fetch_sha256sums constructs the
        // URL as "{base_url}/jms-bridge-{version}-SHA256SUMS". Verify the
        // naming matches what CI produces.
        let version = "0.2.0";
        let base = format!(
            "https://github.com/kennycallado/rust-camel/releases/download/jms-bridge-v{version}"
        );
        let expected = format!("{base}/jms-bridge-{version}-SHA256SUMS");

        // The actual URL is built inside fetch_sha256sums:
        //   format!("{base_url}/jms-bridge-{version}-SHA256SUMS")
        // where base_url comes from resolve_release_base_url.
        let resolved_base = resolve_release_base_url(None, version).unwrap();
        let actual = format!("{resolved_base}/jms-bridge-{version}-SHA256SUMS");

        assert_eq!(
            actual, expected,
            "SHA256SUMS URL must use version-prefixed filename matching CI upload"
        );
    }

    fn build_test_tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use tar::{Builder, Header};

        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = Builder::new(encoder);
        for (path, data) in entries {
            let mut header = Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, *path, *data)
                .expect("append tar entry should succeed");
        }
        let encoder = builder.into_inner().expect("finish tar builder");
        encoder.finish().expect("finish gzip")
    }

    fn build_raw_tarball_with_path(path: &str, data: &[u8]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let mut tar_bytes = Vec::new();
        let mut header = [0u8; 512];

        let path_bytes = path.as_bytes();
        let name_len = path_bytes.len().min(100);
        header[..name_len].copy_from_slice(&path_bytes[..name_len]);

        // mode, uid, gid
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");

        // size (octal) and mtime
        let size_octal = format!("{:011o}\0", data.len());
        header[124..136].copy_from_slice(size_octal.as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");

        // checksum field as spaces for checksum calculation
        for b in &mut header[148..156] {
            *b = b' ';
        }

        // typeflag + ustar metadata
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");

        let checksum: u32 = header.iter().map(|b| *b as u32).sum();
        let checksum_octal = format!("{:06o}\0 ", checksum);
        header[148..156].copy_from_slice(checksum_octal.as_bytes());

        tar_bytes.extend_from_slice(&header);
        tar_bytes.extend_from_slice(data);

        let pad = (512 - (data.len() % 512)) % 512;
        if pad > 0 {
            tar_bytes.extend(std::iter::repeat(0u8).take(pad));
        }

        // end-of-archive marker (two empty blocks)
        tar_bytes.extend(std::iter::repeat(0u8).take(1024));

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        use std::io::Write;
        encoder.write_all(&tar_bytes).expect("write gzip payload");
        encoder.finish().expect("finish gzip")
    }

    #[test]
    fn unpack_tarball_rejects_path_traversal() {
        let bytes = build_raw_tarball_with_path("../../etc/passwd", b"owned");
        let dir = std::env::temp_dir().join(format!(
            "camel-bridge-unpack-reject-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let err = unpack_tarball(&bytes, &dir).unwrap_err();
        assert!(
            err.to_string().contains("path traversal"),
            "expected traversal rejection, got: {err}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unpack_tarball_rejects_absolute_path() {
        let bytes = build_raw_tarball_with_path("/etc/passwd", b"owned");
        let dir = std::env::temp_dir().join(format!(
            "camel-bridge-unpack-reject-abs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let err = unpack_tarball(&bytes, &dir).unwrap_err();
        assert!(
            err.to_string().contains("path traversal"),
            "expected traversal rejection, got: {err}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unpack_tarball_allows_safe_relative_paths() {
        let bytes = build_test_tarball(&[("bin/jms-bridge", b"binary")]);
        let dir = std::env::temp_dir().join(format!(
            "camel-bridge-unpack-ok-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        unpack_tarball(&bytes, &dir).unwrap();

        let extracted = dir.join("bin").join("jms-bridge");
        assert!(extracted.exists(), "expected extracted file to exist");
        assert_eq!(std::fs::read(&extracted).unwrap(), b"binary");

        std::fs::remove_dir_all(&dir).ok();
    }
}
