//! Ephemeral mTLS certificate material for bridge IPC.
//! Generated per-spawn via rcgen, never persisted beyond process lifetime.

use std::path::{Path, PathBuf};

use rcgen::{CertificateParams, DnType, SanType};
use tempfile::TempDir;

use crate::process::BridgeError;

const TLS_DIR_PREFIX: &str = "camel-bridge-tls-";

/// Resolve a writable base directory for ephemeral TLS material.
///
/// Rung order: `$TMPDIR` (when set and non-empty), the platform temp dir, then
/// `{cache_dir}/{version}/tls`. Each candidate is probed by attempting the real
/// `tempdir_in` creation. The returned path is the base; callers create their
/// own unique subdirectory inside it.
pub(crate) fn resolve_tls_base() -> Result<PathBuf, BridgeError> {
    let mut tried: Vec<String> = Vec::new();

    if let Some(tmpdir) = std::env::var_os("TMPDIR").filter(|value| !value.is_empty()) {
        let candidate = PathBuf::from(tmpdir);
        if probe_tempdir(&candidate).is_ok() {
            return Ok(candidate);
        }
        tried.push(format!("$TMPDIR={}", candidate.display()));
    } else {
        tried.push("$TMPDIR (unset)".to_string());
    }

    let system = std::env::temp_dir();
    if probe_tempdir(&system).is_ok() {
        return Ok(system);
    }
    tried.push(format!("system temp {}", system.display()));

    let cache = dirs::cache_dir().map(|dir| dir.join(env!("CARGO_PKG_VERSION")).join("tls"));
    if let Some(candidate) = cache {
        if ensure_private_dir(&candidate).is_ok() && probe_tempdir(&candidate).is_ok() {
            return Ok(candidate);
        }
        tried.push(format!("cache dir {}", candidate.display()));
    } else {
        tried.push("cache dir (unavailable: no platform cache directory)".to_string());
    }

    Err(BridgeError::Config(format!(
        "bridge TLS material: no writable directory (tried {}); set XDG_CACHE_HOME to a writable directory or make $TMPDIR writable",
        tried.join(", ")
    )))
}

/// Probe `base` by creating and immediately dropping a temporary directory.
fn probe_tempdir(base: &Path) -> std::io::Result<()> {
    tempfile::Builder::new()
        .prefix(TLS_DIR_PREFIX)
        .tempdir_in(base)
        .map(|_probe| ())
}

/// Create the cache rung's directory chain (`{cache_root}/{version}/tls`) one
/// level at a time. Every component we create gets mode `0700` (unix only;
/// other platforms use the default ACLs). A pre-existing directory keeps its
/// mode and must actually be a directory.
fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    let mut components = Vec::with_capacity(3);
    if let Some(version_dir) = path.parent() {
        if let Some(cache_root) = version_dir.parent() {
            components.push(cache_root);
        }
        components.push(version_dir);
    }
    components.push(path);
    for component in components {
        create_private_component(component)?;
    }
    Ok(())
}

/// `create_dir` one level, chmod `0700` only when this call created it.
fn create_private_component(path: &Path) -> std::io::Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            if std::fs::metadata(path)?.is_dir() {
                Ok(())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("{} exists but is not a directory", path.display()),
                ))
            }
        }
        Err(err) => Err(err),
    }
}

pub(crate) struct BridgeTlsMaterial {
    pub ca_pem: Vec<u8>,
    pub client_pem: Vec<u8>,
    pub client_key_pem: Vec<u8>,
    pub server_pem_path: PathBuf,
    pub server_key_path: PathBuf,
    pub ca_pem_path: PathBuf,
    _temp_dir: TempDir,
}

impl BridgeTlsMaterial {
    pub(crate) fn generate() -> Result<Self, BridgeError> {
        use rcgen::{BasicConstraints, IsCa, KeyPair};
        use std::net::IpAddr;
        use time::{Duration, OffsetDateTime};

        let now = OffsetDateTime::now_utc();
        let not_before = now.checked_sub(Duration::seconds(60)).unwrap_or(now);
        let not_after = now.checked_add(Duration::days(90)).unwrap_or(now);

        // --- CA ---
        let ca_key =
            KeyPair::generate().map_err(|e| BridgeError::Config(format!("CA keygen: {e}")))?;
        let mut ca_params = CertificateParams::default();
        ca_params.not_before = not_before;
        ca_params.not_after = not_after;
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "camel-bridge CA");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_cert = ca_params
            .self_signed(&ca_key)
            .map_err(|e| BridgeError::Config(format!("CA self-sign: {e}")))?;
        // rcgen 0.14: leaf certs sign via an Issuer built from CA params + key.
        let ca_issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);

        // --- Server cert ---
        let server_key =
            KeyPair::generate().map_err(|e| BridgeError::Config(format!("server keygen: {e}")))?;
        let mut server_params = CertificateParams::default();
        server_params.not_before = not_before;
        server_params.not_after = not_after;
        let localhost = "localhost"
            .try_into()
            .map_err(|e| BridgeError::Config(format!("localhost SAN: {e}")))?;
        server_params.subject_alt_names = vec![
            SanType::DnsName(localhost),
            SanType::IpAddress(IpAddr::V4([127, 0, 0, 1].into())),
            SanType::IpAddress(
                "::1"
                    .parse()
                    .map_err(|e| BridgeError::Config(format!("::1 parse: {e}")))?,
            ),
        ];
        let server_cert = server_params
            .signed_by(&server_key, &ca_issuer)
            .map_err(|e| BridgeError::Config(format!("server cert sign: {e}")))?;

        // --- Client cert ---
        let client_key =
            KeyPair::generate().map_err(|e| BridgeError::Config(format!("client keygen: {e}")))?;
        let mut client_params = CertificateParams::default();
        client_params.not_before = not_before;
        client_params.not_after = not_after;
        client_params
            .distinguished_name
            .push(DnType::CommonName, "camel-bridge client");
        let client_cert = client_params
            .signed_by(&client_key, &ca_issuer)
            .map_err(|e| BridgeError::Config(format!("client cert sign: {e}")))?;

        // --- Write PEMs to a unique TempDir inside the resolved base ---
        let base = resolve_tls_base()?;
        #[cfg(unix)]
        let temp_dir = {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::Builder::new()
                .prefix(TLS_DIR_PREFIX)
                .tempdir_in(&base)
                .map_err(|e| BridgeError::Config(format!("tempdir in {}: {e}", base.display())))?;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(|e| BridgeError::Config(format!("tempdir chmod: {e}")))?;
            dir
        };
        #[cfg(not(unix))]
        let temp_dir = tempfile::Builder::new()
            .prefix(TLS_DIR_PREFIX)
            .tempdir_in(&base)
            .map_err(|e| BridgeError::Config(format!("tempdir in {}: {e}", base.display())))?;

        let ca_pem = ca_cert.pem().into_bytes();
        let server_pem = server_cert.pem().into_bytes();
        let server_key_pem = server_key.serialize_pem().into_bytes();
        let client_pem = client_cert.pem().into_bytes();
        let client_key_pem = client_key.serialize_pem().into_bytes();

        let ca_pem_path = write_pem(&temp_dir, "ca.pem", &ca_pem)?;
        let server_pem_path = write_pem(&temp_dir, "server.pem", &server_pem)?;
        let server_key_path = write_pem(&temp_dir, "server-key.pem", &server_key_pem)?;

        Ok(Self {
            ca_pem,
            client_pem,
            client_key_pem,
            server_pem_path,
            server_key_path,
            ca_pem_path,
            _temp_dir: temp_dir,
        })
    }
}

#[cfg(unix)]
fn write_pem(dir: &TempDir, name: &str, content: &[u8]) -> Result<PathBuf, BridgeError> {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.path().join(name);
    std::fs::write(&path, content)
        .map_err(|e| BridgeError::Config(format!("write {name}: {e}")))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| BridgeError::Config(format!("chmod {name}: {e}")))?;
    Ok(path)
}

#[cfg(not(unix))]
fn write_pem(dir: &TempDir, name: &str, content: &[u8]) -> Result<PathBuf, BridgeError> {
    let path = dir.path().join(name);
    std::fs::write(&path, content)
        .map_err(|e| BridgeError::Config(format!("write {name}: {e}")))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_produces_valid_material() {
        let _env = camel_component_api::test_support::EnvGuard::acquire();
        let tls = BridgeTlsMaterial::generate().expect("generate");
        assert!(!tls.ca_pem.is_empty());
        assert!(!tls.client_pem.is_empty());
        assert!(!tls.client_key_pem.is_empty());
        assert!(tls.server_pem_path.exists());
        assert!(tls.server_key_path.exists());
        assert!(tls.ca_pem_path.exists());
    }

    #[test]
    fn server_cert_contains_127_0_0_1_san() {
        // Parse the server cert PEM and verify SAN contains 127.0.0.1
        let _env = camel_component_api::test_support::EnvGuard::acquire();
        let tls = BridgeTlsMaterial::generate().expect("generate");
        let server_pem = std::fs::read_to_string(&tls.server_pem_path).expect("read server.pem");
        let pem_blocks: Vec<_> = rustls_pemfile::certs(&mut server_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .expect("parse pem");
        assert!(!pem_blocks.is_empty());
        let cert_der = &pem_blocks[0];

        // Use x509-parser to extract SANs
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der).expect("parse x509");
        let san_ext = cert
            .subject_alternative_name()
            .expect("SAN extension extraction failed")
            .expect("SAN extension present");
        let general_names = san_ext.value.general_names.iter().collect::<Vec<_>>();
        let has_ip = general_names.iter().any(|gn| {
            matches!(gn, x509_parser::extensions::GeneralName::IPAddress(addr)
                if addr.len() == 4 && addr == &[127, 0, 0, 1])
        });
        assert!(has_ip, "server cert SAN must contain 127.0.0.1");
    }

    #[cfg(unix)]
    #[test]
    fn temp_files_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let _env = camel_component_api::test_support::EnvGuard::acquire();
        let tls = BridgeTlsMaterial::generate().expect("generate");
        let perms = std::fs::metadata(&tls.server_key_path)
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(perms & 0o777, 0o600);
    }

    #[test]
    fn drop_cleans_up_temp_dir() {
        let _env = camel_component_api::test_support::EnvGuard::acquire();
        let path;
        {
            let tls = BridgeTlsMaterial::generate().expect("generate");
            path = tls.server_pem_path.clone();
            assert!(path.exists());
        }
        assert!(!path.exists(), "temp file should be cleaned up on Drop");
    }

    /// Env marker marking the isolated TMPDIR rung child process.
    const TLS_RUNG_ISOLATION_CHILD_ENV: &str = "CAMEL_BRIDGE_TLS_RUNG_ISOLATION_CHILD";
    /// Child scenario selector for the isolated TMPDIR rung tests.
    const TLS_RUNG_ISOLATION_SCENARIO_ENV: &str = "CAMEL_BRIDGE_TLS_RUNG_SCENARIO";
    /// Expected resolved TLS base passed to the `tmpdir-writable` child.
    const TLS_RUNG_ISOLATION_EXPECT_ENV: &str = "CAMEL_BRIDGE_TLS_RUNG_EXPECT";
    /// Full test path spawned for the isolated TMPDIR rung runs.
    const TLS_RUNG_ISOLATION_TEST: &str = "tls::tests::tls_rung_isolation_inner";

    /// Spawn the test binary with a single-test envelope so a rung scenario can
    /// mutate its own process environment without racing the parallel suite.
    fn spawn_rung_child(scenario: &str, configure: impl FnOnce(&mut std::process::Command)) {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => panic!("current test executable path: {e}"),
        };
        let mut command = std::process::Command::new(exe);
        command
            .args([
                "--exact",
                TLS_RUNG_ISOLATION_TEST,
                "--test-threads=1",
                "--nocapture",
            ])
            .env(TLS_RUNG_ISOLATION_CHILD_ENV, "1")
            .env(TLS_RUNG_ISOLATION_SCENARIO_ENV, scenario);
        configure(&mut command);
        let output = match command.output() {
            Ok(output) => output,
            Err(e) => panic!("spawn isolated TMPDIR rung test: {e}"),
        };
        assert!(
            output.status.success(),
            "isolated TMPDIR rung child failed: scenario={scenario} status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    /// Child body for the isolated TMPDIR rung scenarios. Inert unless the
    /// parent set the marker, so it is a no-op in normal suite runs.
    #[test]
    fn tls_rung_isolation_inner() {
        if std::env::var(TLS_RUNG_ISOLATION_CHILD_ENV).ok().as_deref() != Some("1") {
            return;
        }
        let scenario = std::env::var(TLS_RUNG_ISOLATION_SCENARIO_ENV)
            .unwrap_or_else(|e| panic!("child scenario env: {e}"));
        match scenario.as_str() {
            "tmpdir-writable" => {
                let expected = PathBuf::from(
                    std::env::var_os(TLS_RUNG_ISOLATION_EXPECT_ENV)
                        .expect("child expect env missing"),
                );
                let resolved = resolve_tls_base().expect("resolve");
                assert_eq!(resolved, expected);
            }
            "tmpdir-unset" => {
                let resolved = resolve_tls_base().expect("resolve");
                let expected = std::env::temp_dir();
                assert_eq!(resolved, expected);
            }
            "cache-dir-rung" => {
                let cache_root = expected_cache_root();

                let base = resolve_tls_base().expect("cache rung");
                let expected = cache_root.join(env!("CARGO_PKG_VERSION")).join("tls");
                assert_eq!(base, expected);

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = std::fs::metadata(&base)
                        .expect("stat tls dir")
                        .permissions()
                        .mode();
                    assert_eq!(mode & 0o777, 0o700, "tls leaf must be 0700");
                    let version_dir = base.parent().expect("version dir");
                    let version_mode = std::fs::metadata(version_dir)
                        .expect("stat version dir")
                        .permissions()
                        .mode();
                    assert_eq!(version_mode & 0o777, 0o700, "version dir must be 0700");
                    let cache_mode = std::fs::metadata(&cache_root)
                        .expect("stat cache root")
                        .permissions()
                        .mode();
                    assert_eq!(cache_mode & 0o777, 0o700, "cache root must be 0700");
                }

                let tls = BridgeTlsMaterial::generate().expect("generate");
                assert!(tls.server_pem_path.starts_with(&base));
                let spawn_dir = tls.server_pem_path.parent().expect("spawn dir");
                assert_ne!(spawn_dir, base.as_path(), "spawn dir must be unique");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = std::fs::metadata(spawn_dir)
                        .expect("stat spawn dir")
                        .permissions()
                        .mode();
                    assert_eq!(mode & 0o777, 0o700, "spawn dir must be 0700");
                }
            }
            "all-rungs-fail" => {
                let err = resolve_tls_base().expect_err("all rungs must fail");
                let message = err.to_string();
                assert!(message.contains("XDG_CACHE_HOME"), "message: {message}");
                assert!(message.contains("$TMPDIR"), "message: {message}");
            }
            other => panic!("unknown TMPDIR rung scenario: {other}"),
        }
    }

    #[test]
    fn tmpdir_rung_wins_when_writable() {
        let fresh = tempfile::tempdir().expect("fresh tempdir");
        spawn_rung_child("tmpdir-writable", |command| {
            command
                .env("TMPDIR", fresh.path())
                .env(TLS_RUNG_ISOLATION_EXPECT_ENV, fresh.path());
        });
    }

    #[test]
    fn system_temp_rung_when_tmpdir_unset() {
        spawn_rung_child("tmpdir-unset", |command| {
            command.env_remove("TMPDIR");
        });
    }

    /// Platform cache root that `resolve_tls_base` must use for the temporary
    /// environment. Call this only AFTER the env guard is applied: Linux honours
    /// `XDG_CACHE_HOME`, while macOS ignores it and derives
    /// `$HOME/Library/Caches`. Hard-coding one platform's layout would make the
    /// assertion Linux-only.
    fn expected_cache_root() -> PathBuf {
        dirs::cache_dir().expect("platform cache directory")
    }

    /// Cache root a child sees for the `XDG_CACHE_HOME`/`HOME` values the outer
    /// test exports, mirroring the `dirs` crate rules: Linux honours
    /// `XDG_CACHE_HOME`, while macOS ignores it and derives
    /// `$HOME/Library/Caches`.
    fn child_visible_cache_root(xdg: &Path, home: &Path) -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            let _ = xdg;
            home.join("Library/Caches")
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = home;
            xdg.to_path_buf()
        }
    }

    #[test]
    fn cache_dir_rung_when_no_tmp_usable() {
        let fresh = tempfile::tempdir().expect("fresh tempdir");
        let unusable_tmp = fresh.path().join("no-such-dir");
        let home = fresh.path().join("home");
        std::fs::create_dir_all(&home).expect("create synthetic HOME");
        let xdg = fresh.path().join("xdg-cache");
        let cache_root = child_visible_cache_root(&xdg, &home);
        if let Some(parent) = cache_root.parent() {
            std::fs::create_dir_all(parent).expect("create platform cache ancestors");
        }
        spawn_rung_child("cache-dir-rung", |command| {
            command
                .env("TMPDIR", &unusable_tmp)
                // Linux uses XDG_CACHE_HOME; macOS uses $HOME/Library/Caches.
                .env("XDG_CACHE_HOME", &xdg)
                .env("HOME", &home);
        });
    }

    #[test]
    fn total_failure_names_xdg_override() {
        let fresh = tempfile::tempdir().expect("fresh tempdir");
        let blocker = fresh.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").expect("write blocker");
        let unusable = blocker.join("child");
        let unusable_tmp = fresh.path().join("no-such-dir");
        spawn_rung_child("all-rungs-fail", |command| {
            command
                .env("TMPDIR", &unusable_tmp)
                .env("XDG_CACHE_HOME", &unusable)
                .env("HOME", &unusable);
        });
    }
}
