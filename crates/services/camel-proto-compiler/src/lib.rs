//! In-process `.proto` compilation with import-aware content caching.
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

use prost_reflect::DescriptorPool;

pub use cache::ProtoCache;
pub use compiler::compile_proto;
pub use embedded::EMBEDDED_REF_PREFIX;
use sha2::{Digest, Sha256};

/// Returns the absolute local filesystem paths of the transitive imports of
/// `proto_path`, sorted and deduplicated.
///
/// Imports resolve against the schema's parent directory (the same rule
/// `compile_proto` applies when no include paths are supplied), then Google
/// well-known types. The root schema itself is not part of the result; neither
/// are well-known or embedded inputs, which have no disk path. A descriptor
/// set carries all its imports inline, so its closure is empty.
///
/// An unresolvable import fails with the same [`ProtoCompileError::Compile`]
/// variant `compile_proto` reports for a missing include.
pub fn local_import_closure(
    proto_path: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, ProtoCompileError> {
    // Mirror `compile_proto`'s fail-closed root checks: an unregistered
    // embedded reference and a missing path are `ProtoNotFound`.
    if let Some(name) = embedded::strip_ref(proto_path) {
        if !embedded::contains(name) {
            return Err(ProtoCompileError::ProtoNotFound(proto_path.to_path_buf()));
        }
        // Embedded inputs have no disk path, so the local closure is empty.
        return Ok(Vec::new());
    }
    if !proto_path.exists() {
        return Err(ProtoCompileError::ProtoNotFound(proto_path.to_path_buf()));
    }
    if compiler::is_descriptor_set(proto_path) {
        return Ok(Vec::new());
    }

    let (snapshot, local) = compiler::preload_local_snapshot_contained(proto_path, &[])?;
    if snapshot.has_missing() {
        // Surface protox's canonical missing-import diagnostic, exactly as
        // the compile path does.
        let _ = compiler::compile_from_snapshot(proto_path, snapshot)?;
        unreachable!("a snapshot with a missing import must not compile");
    }
    Ok(local)
}

/// Compiles `proto_path` and returns the descriptor pool together with the
/// local import closure, both derived from ONE source snapshot.
///
/// The pool and the path list are read from the same preloaded graph, so an
/// edit that lands while the call is running can never produce a descriptor
/// built from one import set and a source list from another. Non-source inputs
/// (embedded refs and precompiled descriptor sets) carry no local imports and
/// report an empty list.
pub fn compile_proto_with_sources(
    proto_path: &Path,
) -> Result<(DescriptorPool, Vec<PathBuf>), ProtoCompileError> {
    // Embedded refs and descriptor sets have no on-disk import closure;
    // `compile_proto` already owns their fail-closed checks.
    if embedded::strip_ref(proto_path).is_some()
        || !proto_path.exists()
        || compiler::is_descriptor_set(proto_path)
    {
        let pool = compile_proto(proto_path, std::iter::empty::<&Path>())?;
        return Ok((pool, Vec::new()));
    }
    compiler::contained_value(proto_path, || {
        let (snapshot, local) = compiler::preload_local_snapshot(proto_path, &[])?;
        let pool = compiler::compile_from_snapshot(proto_path, snapshot)?;
        Ok((pool, local))
    })
}

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

    fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("write proto");
        path
    }

    fn abs(path: PathBuf) -> PathBuf {
        std::path::absolute(&path).expect("absolute path")
    }

    #[test]
    fn local_import_closure_transitive() {
        let tmp = TempDir::new().expect("tmp dir");
        write(
            tmp.path(),
            "a.proto",
            "syntax = \"proto3\";\npackage a;\nimport \"b.proto\";\nmessage A { b.B b = 1; }\n",
        );
        write(
            tmp.path(),
            "b.proto",
            "syntax = \"proto3\";\npackage b;\nimport \"c.proto\";\nmessage B { c.C c = 1; }\n",
        );
        write(
            tmp.path(),
            "c.proto",
            "syntax = \"proto3\";\npackage c;\nmessage C { string v = 1; }\n",
        );

        let closure =
            local_import_closure(&tmp.path().join("a.proto")).expect("closure should resolve");

        let expected = vec![
            abs(tmp.path().join("b.proto")),
            abs(tmp.path().join("c.proto")),
        ];
        assert_eq!(
            closure, expected,
            "transitive imports sorted, root excluded"
        );
    }

    #[test]
    fn local_import_closure_excludes_well_known() {
        let tmp = TempDir::new().expect("tmp dir");
        write(
            tmp.path(),
            "root.proto",
            "syntax = \"proto3\";\npackage wkt_closure;\nimport \"google/protobuf/empty.proto\";\nimport \"dep.proto\";\nmessage Root { dep.Dep d = 1; google.protobuf.Empty e = 2; }\n",
        );
        write(
            tmp.path(),
            "dep.proto",
            "syntax = \"proto3\";\npackage dep;\nmessage Dep { string v = 1; }\n",
        );

        let closure =
            local_import_closure(&tmp.path().join("root.proto")).expect("closure should resolve");

        assert_eq!(closure, vec![abs(tmp.path().join("dep.proto"))]);
        assert!(
            closure
                .iter()
                .all(|p| !p.to_string_lossy().contains("google/protobuf")),
            "well-known inputs must be excluded: {closure:?}"
        );
    }

    #[test]
    fn local_import_closure_missing_import_errs() {
        let tmp = TempDir::new().expect("tmp dir");
        let root = write(
            tmp.path(),
            "root.proto",
            "syntax = \"proto3\";\npackage missing_closure;\nimport \"missing.proto\";\nmessage Root { string v = 1; }\n",
        );

        let closure_err =
            local_import_closure(&root).expect_err("a missing import must fail the closure");
        let compile_err = compile_proto(&root, std::iter::once(tmp.path()))
            .expect_err("a missing import must fail compile_proto");

        assert!(
            matches!(closure_err, ProtoCompileError::Compile { .. }),
            "closure error must be Compile, got {closure_err:?}"
        );
        assert!(
            matches!(compile_err, ProtoCompileError::Compile { .. }),
            "compile_proto error must be Compile, got {compile_err:?}"
        );
    }

    #[test]
    fn local_import_closure_matches_cache_invalidation_set() {
        let tmp = TempDir::new().expect("tmp dir");
        let root_dir = tmp.path().join("root");
        let shadow_dir = tmp.path().join("shadow");
        std::fs::create_dir_all(&root_dir).expect("create root dir");
        std::fs::create_dir_all(&shadow_dir).expect("create shadow dir");

        // root_dir's common.proto wins; shadow_dir's copy is the shadowed
        // include from the cache fixture.
        write(
            &shadow_dir,
            "common.proto",
            "syntax = \"proto3\";\npackage common;\nmessage Common { string shadow = 1; }\n",
        );
        let root_common = write(
            &root_dir,
            "common.proto",
            "syntax = \"proto3\";\npackage common;\nmessage Common { string winner = 1; }\n",
        );
        let leaf = write(
            &root_dir,
            "leaf.proto",
            "syntax = \"proto3\";\npackage leaf;\nmessage Leaf { string v = 1; }\n",
        );
        let mid = write(
            &root_dir,
            "mid.proto",
            "syntax = \"proto3\";\npackage mid;\nimport \"leaf.proto\";\nmessage Mid { leaf.Leaf l = 1; }\n",
        );
        let root = write(
            &root_dir,
            "root.proto",
            "syntax = \"proto3\";\npackage root;\nimport \"common.proto\";\nimport \"mid.proto\";\nmessage Root { common.Common c = 1; mid.Mid m = 2; }\n",
        );

        let closure = local_import_closure(&root).expect("closure should resolve");
        let mut expected = vec![
            abs(root_common.clone()),
            abs(leaf.clone()),
            abs(mid.clone()),
        ];
        expected.sort();
        assert_eq!(closure, expected);

        let includes = [root_dir.as_path(), shadow_dir.as_path()];
        for file in &closure {
            let cache = ProtoCache::new();
            cache
                .get_or_compile(&root, includes)
                .expect("initial compile");
            assert_eq!(cache.len(), 1);

            let mut contents = std::fs::read_to_string(file).expect("read closure file");
            contents.push_str("// touched\n");
            std::fs::write(file, contents).expect("mutate closure file");

            cache
                .get_or_compile(&root, includes)
                .expect("compile after mutation");
            assert_eq!(
                cache.len(),
                2,
                "mutating {file:?} must invalidate the cache entry"
            );
        }

        // The shadowed include is not part of the closure and its mutation is
        // invisible to the cache.
        let shadowed_common = abs(shadow_dir.join("common.proto"));
        assert!(!closure.contains(&shadowed_common));
        let cache = ProtoCache::new();
        cache
            .get_or_compile(&root, includes)
            .expect("initial compile before shadow mutation");
        let mut contents = std::fs::read_to_string(&shadowed_common).expect("read shadow file");
        contents.push_str("// touched\n");
        std::fs::write(&shadowed_common, contents).expect("mutate shadow file");
        cache
            .get_or_compile(&root, includes)
            .expect("compile after shadow mutation");
        assert_eq!(
            cache.len(),
            1,
            "mutating the shadowed include must not invalidate the cache entry"
        );
    }

    const ROOT_WITHOUT_IMPORT: &str =
        "syntax = \"proto3\";\npackage reload;\nmessage M { string a = 1; }\n";
    const ROOT_WITH_IMPORT: &str = "syntax = \"proto3\";\npackage reload;\nimport \"common.proto\";\nmessage M { string a = 1; common.B b = 2; }\n";

    /// An edit that lands between the source read and the compile must not
    /// split the descriptor from its source list: both stay on the snapshot
    /// taken before the edit.
    #[test]
    fn combined_compile_keeps_sources_consistent_across_intervening_edit() {
        let tmp = TempDir::new().expect("tmp dir");
        let common = write(
            tmp.path(),
            "common.proto",
            "syntax = \"proto3\";\npackage common;\nmessage B { string v = 1; }\n",
        );
        let root = write(tmp.path(), "root.proto", ROOT_WITHOUT_IMPORT);

        // Seam: acquire the snapshot, then let the edit land.
        let (snapshot, local) =
            compiler::preload_local_snapshot(&root, &[]).expect("snapshot should resolve");
        assert!(local.is_empty(), "pre-edit snapshot has no local imports");
        std::fs::write(&root, ROOT_WITH_IMPORT).expect("edit root after the snapshot");

        let pool = compiler::compile_from_snapshot(&root, snapshot).expect("compile snapshot");
        let message = pool.get_message_by_name("reload.M").expect("message M");
        assert!(
            message.get_field_by_name("b").is_none(),
            "the descriptor must come from the pre-edit snapshot"
        );
        assert!(
            local.is_empty(),
            "sources must match the pre-edit descriptor, got {local:?}"
        );

        // After the edit, the combined helper moves both sides together.
        let (pool, local) = compile_proto_with_sources(&root).expect("combined compile");
        let message = pool.get_message_by_name("reload.M").expect("message M");
        assert!(
            message.get_field_by_name("b").is_some(),
            "the descriptor must include the new import"
        );
        assert_eq!(
            local,
            vec![abs(common)],
            "sources must include the same import the descriptor used"
        );
    }
}
