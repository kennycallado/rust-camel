//! Runtime `.proto` file compilation with SHA-256 content caching.
//!
//! Protoc resolution order: the `PROTOC` environment override first
//! (honored verbatim, never re-resolved), the vendored `protoc`
//! fallback second, and a typed `ProtoCompileError::ProtocUnavailable`
//! failure when neither yields a binary.
//!
//! Main types: `ProtoCache`, `ProtoCompileError`, `compile_proto`.
//! Main modules: `cache`, `compiler`.

mod cache;
mod compiler;

use std::path::{Path, PathBuf};

pub use cache::ProtoCache;
pub use compiler::compile_proto;
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum ProtoCompileError {
    #[error("proto file not found: {0}")]
    ProtoNotFound(PathBuf),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "protoc unavailable: {detail}. Set PROTOC to a protoc binary, or use a build where the vendored protoc is present"
    )]
    ProtocUnavailable { detail: String },
    #[error("protoc failed (status: {status:?}): {stderr}")]
    ProtocFailed { status: Option<i32>, stderr: String },
    #[error("failed to decode descriptor pool: {0}")]
    DescriptorDecode(String),
}

fn hash_proto_content(path: &Path) -> Result<String, ProtoCompileError> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::*;
    use crate::compiler::{resolve_protoc, resolve_protoc_with};

    /// Serializes tests that invoke `compile_proto` (protoc resolution:
    /// `PROTOC` override first, then the vendored fallback through
    /// `protoc_bin_vendored::protoc_bin_path()`). The vendored protoc
    /// extracts to a deterministic temp path; parallel extraction under
    /// test contention races and intermittently fails. This lock
    /// serializes only the affected tests (rc-alwn).
    static PROTOC_COMPILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Saves the `PROTOC` environment variable on construction and
    /// restores it (set or removed) on drop. The caller MUST hold
    /// `PROTOC_COMPILE_LOCK` while creating and dropping the guard;
    /// the constructor and `Drop` do not acquire the lock themselves
    /// (same-thread re-lock would deadlock). Declare the guard after
    /// the lock binding so `Drop` runs before the lock is released.
    struct ProtocEnvGuard(Option<std::ffi::OsString>);

    impl ProtocEnvGuard {
        fn new() -> Self {
            // Caller holds PROTOC_COMPILE_LOCK, which serializes all
            // PROTOC access across tests.
            Self(std::env::var_os("PROTOC"))
        }
    }

    impl Drop for ProtocEnvGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => {
                    // SAFETY: PROTOC_COMPILE_LOCK (held by the caller,
                    // still held here because the guard is declared after
                    // the lock binding) serializes all PROTOC access.
                    unsafe { std::env::set_var("PROTOC", value) };
                }
                None => {
                    // SAFETY: PROTOC_COMPILE_LOCK (held by the caller,
                    // still held here because the guard is declared after
                    // the lock binding) serializes all PROTOC access.
                    unsafe { std::env::remove_var("PROTOC") };
                }
            }
        }
    }

    fn test_proto_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("helloworld.proto")
    }

    /// Writes an executable shell script `fake-protoc` into `dir`. The
    /// script appends `invoked` to a `marker` file next to itself and,
    /// for each `--descriptor_set_out=<path>` argument, copies the
    /// committed `tests/helloworld.desc` fixture to `<path>`. Returns
    /// the script path for use as the `PROTOC` override.
    fn write_fake_protoc(dir: &Path) -> PathBuf {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("helloworld.desc")
            .display()
            .to_string();
        let script_path = dir.join("fake-protoc");
        let script = format!(
            r#"#!/bin/sh
set -eu
printf 'invoked\n' >> "$(dirname "$0")/marker"
for arg in "$@"; do
  case "$arg" in
    --descriptor_set_out=*)
      cp "{fixture}" "${{arg#--descriptor_set_out=}}"
      ;;
  esac
done
"#,
            fixture = fixture,
        );
        std::fs::write(&script_path, script).expect("write fake protoc script");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("make fake protoc executable");
        script_path
    }

    #[test]
    fn compile_proto_success() {
        let _guard = PROTOC_COMPILE_LOCK.lock().unwrap();
        let proto = test_proto_path();
        let pool =
            compile_proto(&proto, std::iter::empty::<&Path>()).expect("compile should succeed");
        assert!(
            pool.get_message_by_name("helloworld.HelloRequest")
                .is_some()
        );
        assert!(pool.get_service_by_name("helloworld.Greeter").is_some());
    }

    #[test]
    fn compile_proto_missing_file_returns_error() {
        let err = compile_proto("/definitely/missing.proto", std::iter::empty::<&Path>())
            .expect_err("should fail");
        assert!(matches!(err, ProtoCompileError::ProtoNotFound(_)));
    }

    #[test]
    fn compile_proto_invalid_syntax_returns_error() {
        let _guard = PROTOC_COMPILE_LOCK.lock().unwrap();
        let tmp = TempDir::new().expect("tmp dir");
        let bad_proto = tmp.path().join("bad.proto");
        std::fs::write(
            &bad_proto,
            "syntax = \"proto3\";\nmessage Broken { string x = ; }\n",
        )
        .expect("write invalid proto");

        let err = compile_proto(&bad_proto, std::iter::once(tmp.path())).expect_err("should fail");
        assert!(matches!(err, ProtoCompileError::ProtocFailed { .. }));
    }

    #[test]
    fn cache_hit_does_not_duplicate_entries() {
        let _guard = PROTOC_COMPILE_LOCK.lock().unwrap();
        let cache = ProtoCache::new();
        let proto = test_proto_path();

        let p1 = cache
            .get_or_compile(&proto, std::iter::empty::<&Path>())
            .expect("first compile should succeed");
        let p2 = cache
            .get_or_compile(&proto, std::iter::empty::<&Path>())
            .expect("second compile should hit cache");

        assert!(p1.get_service_by_name("helloworld.Greeter").is_some());
        assert!(p2.get_service_by_name("helloworld.Greeter").is_some());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_does_not_grow_beyond_max() {
        let _guard = PROTOC_COMPILE_LOCK.lock().unwrap();
        let cache = ProtoCache::with_max_entries(3);
        let tmp = tempfile::tempdir().expect("tmp dir");

        // Write 4 different proto files, each with a different package name.
        for i in 0..4 {
            let proto = tmp.path().join(format!("pkg{i}.proto"));
            std::fs::write(
                &proto,
                format!(r#"syntax = "proto3"; package pkg{i}; message M {{ string name = 1; }}"#),
            )
            .expect("write proto");
            cache
                .get_or_compile(&proto, std::iter::once(tmp.path() as &Path))
                .expect("compile should succeed");
        }

        assert!(
            cache.len() <= 3,
            "cache should respect max_entries, got {}",
            cache.len()
        );
    }

    #[test]
    fn test_concurrent_compiles_do_not_clobber() {
        let _guard = PROTOC_COMPILE_LOCK.lock().unwrap();
        let proto = test_proto_path();
        let mut handles = Vec::new();
        for _ in 0..4 {
            let proto = proto.clone();
            handles.push(std::thread::spawn(move || {
                compile_proto(&proto, std::iter::empty::<&Path>())
            }));
        }
        for handle in handles {
            let result = handle.join().expect("thread panicked");
            let pool = result.expect("concurrent compile should succeed");
            assert!(
                pool.get_message_by_name("helloworld.HelloRequest")
                    .is_some(),
                "concurrent compile produced incomplete descriptor",
            );
        }
    }

    #[test]
    fn test_descriptor_file_cleaned_up() {
        let _guard = PROTOC_COMPILE_LOCK.lock().unwrap();
        let proto = test_proto_path();
        compile_proto(&proto, std::iter::empty::<&Path>()).expect("compile should succeed");

        let entries = std::fs::read_dir(std::env::temp_dir()).expect("read temp dir");
        for entry in entries {
            let entry = entry.expect("read entry");
            let name = entry.file_name();
            if let Some(name) = name.to_str() {
                assert!(
                    !name.starts_with("camel-proto-") || !name.ends_with(".desc"),
                    "leftover descriptor file: {name}",
                );
            }
        }
    }

    #[test]
    fn cache_invalidation_on_content_change() {
        let _guard = PROTOC_COMPILE_LOCK.lock().unwrap();
        let cache = ProtoCache::new();
        let tmp = TempDir::new().expect("tmp dir");
        let proto = tmp.path().join("demo.proto");

        std::fs::write(
            &proto,
            r#"syntax = "proto3";
package demo;
message A { string name = 1; }
"#,
        )
        .expect("write proto v1");

        let pool_v1 = cache
            .get_or_compile(&proto, std::iter::once(tmp.path()))
            .expect("compile v1");
        assert!(pool_v1.get_message_by_name("demo.A").is_some());
        assert_eq!(cache.len(), 1);

        std::fs::write(
            &proto,
            r#"syntax = "proto3";
package demo;
message A { string name = 1; }
message B { int32 id = 1; }
"#,
        )
        .expect("write proto v2");

        let pool_v2 = cache
            .get_or_compile(&proto, std::iter::once(tmp.path()))
            .expect("compile v2");
        assert!(pool_v2.get_message_by_name("demo.B").is_some());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn protoc_env_short_circuits_vendored_resolver() {
        let _lock = PROTOC_COMPILE_LOCK.lock().unwrap();
        let _env = ProtocEnvGuard::new();
        // SAFETY: PROTOC_COMPILE_LOCK (held above) serializes all PROTOC access.
        unsafe { std::env::set_var("PROTOC", "/protofix/fake/protoc") };

        let result = resolve_protoc_with(|| panic!("vendored resolver must not run"));

        assert_eq!(result.ok(), Some(PathBuf::from("/protofix/fake/protoc")));
    }

    #[test]
    fn protoc_env_empty_string_is_honored_as_set() {
        let _lock = PROTOC_COMPILE_LOCK.lock().unwrap();
        let _env = ProtocEnvGuard::new();
        // SAFETY: PROTOC_COMPILE_LOCK (held above) serializes all PROTOC access.
        unsafe { std::env::set_var("PROTOC", "") };

        let result = resolve_protoc_with(|| panic!("vendored resolver must not run"));

        assert_eq!(result.ok(), Some(PathBuf::from("")));
    }

    #[test]
    fn vendored_panic_contained_as_protoc_unavailable() {
        let _lock = PROTOC_COMPILE_LOCK.lock().unwrap();
        let _env = ProtocEnvGuard::new();
        // SAFETY: PROTOC_COMPILE_LOCK (held above) serializes all PROTOC access.
        unsafe { std::env::remove_var("PROTOC") };

        let err = resolve_protoc_with(|| {
            panic!("internal: protoc not found /baked/registry/path/bin/protoc")
        })
        .expect_err("panic must be contained");

        assert!(
            matches!(&err, ProtoCompileError::ProtocUnavailable { detail }
                if detail.contains("internal: protoc not found")),
            "unexpected error: {err:?}"
        );
        assert!(
            err.to_string().contains("Set PROTOC"),
            "display text missing remedy: {err}"
        );
        // The test completing at all is proof the panic was contained.
    }

    #[test]
    fn vendored_err_passes_through_seam_verbatim() {
        let _lock = PROTOC_COMPILE_LOCK.lock().unwrap();
        let _env = ProtocEnvGuard::new();
        // SAFETY: PROTOC_COMPILE_LOCK (held above) serializes all PROTOC access.
        unsafe { std::env::remove_var("PROTOC") };

        let err = resolve_protoc_with(|| {
            Err(ProtoCompileError::ProtocUnavailable {
                detail: "facade unsupported-platform".to_string(),
            })
        })
        .expect_err("should pass through");

        assert!(
            matches!(&err, ProtoCompileError::ProtocUnavailable { detail }
                if detail == "facade unsupported-platform"),
            "seam wrapped or rewrote the closure error: {err:?}"
        );
    }

    #[test]
    fn resolve_protoc_vendored_present_returns_file() {
        let _lock = PROTOC_COMPILE_LOCK.lock().unwrap();
        let _env = ProtocEnvGuard::new();
        // SAFETY: PROTOC_COMPILE_LOCK (held above) serializes all PROTOC access.
        unsafe { std::env::remove_var("PROTOC") };

        let path = resolve_protoc().expect("vendored protoc should resolve");

        assert!(
            path.is_file(),
            "resolved path is not a file: {}",
            path.display()
        );
    }

    #[test]
    fn protoc_env_marker_script_serves_compilation() {
        let _lock = PROTOC_COMPILE_LOCK.lock().unwrap();
        let _env = ProtocEnvGuard::new();
        let tmp = TempDir::new().expect("tmp dir");
        let script = write_fake_protoc(tmp.path());
        let marker = tmp.path().join("marker");
        assert!(!marker.exists(), "marker must not exist before the run");
        // SAFETY: PROTOC_COMPILE_LOCK (held above) serializes all PROTOC access.
        unsafe { std::env::set_var("PROTOC", &script) };

        let proto = test_proto_path();
        let pool = compile_proto(&proto, std::iter::empty::<&Path>())
            .expect("fake protoc should serve the compilation");

        assert!(
            pool.get_message_by_name("helloworld.HelloRequest")
                .is_some(),
            "pool should contain helloworld.HelloRequest"
        );
        assert!(marker.exists(), "fake protoc script was not invoked");
    }

    #[test]
    fn protoc_env_broken_override_fails_without_fallback() {
        let _lock = PROTOC_COMPILE_LOCK.lock().unwrap();
        let _env = ProtocEnvGuard::new();
        // SAFETY: PROTOC_COMPILE_LOCK (held above) serializes all PROTOC access.
        unsafe { std::env::set_var("PROTOC", "/protofix/definitely/missing/protoc") };

        let proto = test_proto_path();
        let err = compile_proto(&proto, std::iter::empty::<&Path>())
            .expect_err("broken PROTOC override must fail");

        assert!(
            matches!(&err, ProtoCompileError::Io(_)),
            "expected Io error from spawn failure, got: {err:?}"
        );
    }
}
