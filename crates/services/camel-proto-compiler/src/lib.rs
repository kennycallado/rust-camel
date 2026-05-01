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
    #[error("vendored protoc unavailable: {0}")]
    VendoredProtoc(#[from] protoc_bin_vendored::Error),
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
        assert!(matches!(err, ProtoCompileError::ProtocFailed { .. }));
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
    fn cache_invalidation_on_content_change() {
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
}
