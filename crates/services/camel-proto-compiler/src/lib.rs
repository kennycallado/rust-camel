//! In-process `.proto` compilation with SHA-256 content caching.
//!
//! Compilation runs inside the process with `protox`; no external compiler is
//! used and nothing is written to a temporary directory. A precompiled
//! descriptor set (`.binpb`, `.pb`, `.desc`, `.protoset`) is accepted wherever
//! a `.proto` path is accepted. Protobuf editions are unsupported.
//!
//! A path with the reserved `camel-embedded:` prefix resolves from the
//! process-global embedded-source registry (`embedded`) instead of the
//! filesystem, with the same hardening as the disk path.
//!
//! Main types: `ProtoCache`, `ProtoCompileError`, `compile_proto`.
//! Main modules: `cache`, `compiler`, `embedded`.

mod cache;
mod compiler;
pub mod embedded;
mod nesting;

use std::path::{Path, PathBuf};

pub use cache::ProtoCache;
pub use compiler::compile_proto;
pub use embedded::EMBEDDED_REF_PREFIX;
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProtoCompileError {
    #[error("proto file not found: {0}")]
    ProtoNotFound(PathBuf),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to compile {}: {detail}", path.display())]
    Compile { path: PathBuf, detail: String },
    #[error("failed to decode descriptor pool: {0}")]
    DescriptorDecode(String),
}

fn hash_proto_content(path: &Path) -> Result<String, ProtoCompileError> {
    let bytes = match compiler::read_schema_bytes(path) {
        Ok(bytes) => bytes,
        Err(compiler::SchemaReadError::Io(err)) => return Err(ProtoCompileError::Io(err)),
        Err(err) => {
            return Err(ProtoCompileError::Compile {
                path: path.to_path_buf(),
                detail: err.detail(path),
            });
        }
    };
    Ok(hash_bytes(&bytes))
}

/// Lowercase-hex SHA-256 over `bytes`; the cache-key content hash for
/// both disk and embedded inputs.
fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::*;

    fn test_proto_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("helloworld.proto")
    }

    #[test]
    fn compile_proto_success() {
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
        let tmp = TempDir::new().expect("tmp dir");
        let bad_proto = tmp.path().join("bad.proto");
        std::fs::write(
            &bad_proto,
            "syntax = \"proto3\";\nmessage Broken { string x = ; }\n",
        )
        .expect("write invalid proto");

        let err = compile_proto(&bad_proto, std::iter::once(tmp.path())).expect_err("should fail");
        assert!(
            matches!(&err, ProtoCompileError::Compile { detail, .. } if detail.contains(":2:")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn cache_hit_does_not_duplicate_entries() {
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
    fn cache_invalidation_on_content_change() {
        let cache = ProtoCache::new();
        let tmp = TempDir::new().expect("tmp dir");
        let proto = tmp.path().join("demo.proto");

        std::fs::write(
            &proto,
            r#"syntax = "proto3";
package demo;
message A { string name = 1; }"#,
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
message B { int32 id = 1; }"#,
        )
        .expect("write proto v2");

        let pool_v2 = cache
            .get_or_compile(&proto, std::iter::once(tmp.path()))
            .expect("compile v2");
        assert!(pool_v2.get_message_by_name("demo.B").is_some());
        assert_eq!(cache.len(), 2);
    }
}
