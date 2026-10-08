//! Error-position and descriptor-set integration tests.
//!
//! These tests pin the protox error text for source errors, the editions
//! remedy, and the typed errors returned for descriptor-set input.

use std::path::{Path, PathBuf};

use camel_proto_compiler::{ProtoCache, ProtoCompileError, compile_proto};
use prost::Message;
use prost_reflect::prost_types::FileDescriptorSet;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(rel: &str) -> PathBuf {
    manifest().join("tests").join("fixtures").join(rel)
}

#[test]
fn syntax_error_has_line_and_column() {
    let err = compile_proto(fixture("err/syntax.proto"), std::iter::empty::<&Path>())
        .expect_err("syntax error must fail");
    match err {
        ProtoCompileError::Compile { detail, .. } => {
            assert!(detail.contains("syntax.proto:2:29"), "detail: {detail}");
        }
        other => panic!("expected Compile error, got {other:?}"),
    }
}

#[test]
fn duplicate_field_number_has_position() {
    let err = compile_proto(fixture("err/dup_number.proto"), std::iter::empty::<&Path>())
        .expect_err("duplicate field number must fail");
    match err {
        ProtoCompileError::Compile { detail, .. } => {
            assert!(detail.contains("dup_number.proto:2:38"), "detail: {detail}");
            assert!(detail.contains("already used"), "detail: {detail}");
        }
        other => panic!("expected Compile error, got {other:?}"),
    }
}

#[test]
fn missing_import_is_compile_error() {
    let err = compile_proto(
        fixture("err/missing_import.proto"),
        std::iter::empty::<&Path>(),
    )
    .expect_err("missing import must fail");
    match err {
        ProtoCompileError::Compile { detail, .. } => {
            assert!(detail.contains("nope/missing.proto"), "detail: {detail}");
        }
        other => panic!("expected Compile error, got {other:?}"),
    }
}

#[test]
fn undefined_type_is_compile_error() {
    let err = compile_proto(
        fixture("err/undefined_type.proto"),
        std::iter::empty::<&Path>(),
    )
    .expect_err("undefined type must fail");
    match err {
        ProtoCompileError::Compile { detail, .. } => {
            assert!(
                detail.contains("undefined_type.proto:2:"),
                "detail: {detail}"
            );
        }
        other => panic!("expected Compile error, got {other:?}"),
    }
}

#[test]
fn proto3_default_and_enum_zero_are_rejected() {
    for rel in ["err/proto3_default.proto", "err/proto3_enum_zero.proto"] {
        let err = compile_proto(fixture(rel), std::iter::empty::<&Path>())
            .expect_err("proto3 restriction must fail");
        assert!(
            matches!(&err, ProtoCompileError::Compile { .. }),
            "{rel}: expected Compile error, got {err:?}"
        );
    }
}

#[test]
fn compile_error_display_names_the_path() {
    let err = compile_proto(fixture("err/syntax.proto"), std::iter::empty::<&Path>())
        .expect_err("syntax error must fail");
    let msg = err.to_string();
    assert!(msg.starts_with("failed to compile "), "msg: {msg}");
    assert!(msg.contains("syntax.proto"), "msg: {msg}");
}

#[test]
fn editions_source_error_names_remedy() {
    let err = compile_proto(fixture("ks/main/ed2023.proto"), std::iter::empty::<&Path>())
        .expect_err("editions source must fail");
    match err {
        ProtoCompileError::Compile { detail, .. } => {
            assert!(detail.contains("editions"), "detail: {detail}");
            assert!(detail.contains("proto3"), "detail: {detail}");
        }
        other => panic!("expected Compile error, got {other:?}"),
    }
}

#[test]
fn editions_descriptor_set_is_typed_error() {
    let err = compile_proto(fixture("golden/ed2023.binpb"), std::iter::empty::<&Path>())
        .expect_err("editions descriptor set must fail");
    match err {
        ProtoCompileError::DescriptorDecode(s) => {
            assert!(s.contains("editions"), "s: {s}");
            assert!(s.contains("ed2023.binpb"), "s: {s}");
        }
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

#[test]
fn precompiled_kitchen_descriptor_set_loads_without_includes() {
    let pool = compile_proto(fixture("golden/kitchen.binpb"), std::iter::empty::<&Path>())
        .expect("kitchen descriptor set must load");
    assert!(
        pool.get_message_by_name("kitchen.v1.Order").is_some(),
        "pool must expose kitchen.v1.Order"
    );
    assert!(
        pool.get_service_by_name("kitchen.v1.OrderService")
            .is_some(),
        "pool must expose kitchen.v1.OrderService"
    );
}

#[test]
fn descriptor_set_extension_is_case_insensitive() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let src = fixture("golden/helloworld.binpb");
    for name in ["HELLO.BINPB", "hello.PB", "hello.desc", "hello.protoset"] {
        let dst = dir.path().join(name);
        std::fs::copy(&src, &dst).expect("copy descriptor set");
        let pool = compile_proto(&dst, std::iter::empty::<&Path>())
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(
            pool.get_message_by_name("helloworld.HelloRequest")
                .is_some(),
            "{name} must expose helloworld.HelloRequest"
        );
    }
}

#[test]
fn descriptor_set_works_through_proto_cache() {
    let cache = ProtoCache::new();
    let path = fixture("golden/helloworld.binpb");
    let p1 = cache
        .get_or_compile(&path, std::iter::empty::<&Path>())
        .expect("first load");
    let p2 = cache
        .get_or_compile(&path, std::iter::empty::<&Path>())
        .expect("second load");
    assert!(p1.get_service_by_name("helloworld.Greeter").is_some());
    assert!(p2.get_service_by_name("helloworld.Greeter").is_some());
    assert_eq!(cache.len(), 1);
}

#[test]
fn corrupt_descriptor_set_names_the_path() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("bad.binpb");
    std::fs::write(&path, [0xff; 5]).expect("write corrupt set");
    let err = compile_proto(&path, std::iter::empty::<&Path>()).expect_err("must fail");
    match err {
        ProtoCompileError::DescriptorDecode(s) => assert!(s.contains("bad.binpb"), "s: {s}"),
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

#[test]
fn missing_descriptor_set_is_not_found() {
    let path = fixture("golden/nope.binpb");
    let err = compile_proto(&path, std::iter::empty::<&Path>()).expect_err("must fail");
    assert!(
        matches!(&err, ProtoCompileError::ProtoNotFound(_)),
        "expected ProtoNotFound, got {err:?}"
    );
}

#[test]
fn descriptor_set_without_imports_is_decode_error() {
    let bytes = std::fs::read(fixture("golden/kitchen.binpb")).expect("read golden set");
    let mut set = FileDescriptorSet::decode(bytes.as_slice()).expect("decode golden set");
    set.file
        .retain(|f| f.name.as_deref() == Some("kitchen.proto"));

    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("partial.binpb");
    std::fs::write(&path, set.encode_to_vec()).expect("write partial set");

    let err = compile_proto(&path, std::iter::empty::<&Path>()).expect_err("must fail");
    assert!(
        matches!(&err, ProtoCompileError::DescriptorDecode(_)),
        "expected DescriptorDecode error, got {err:?}"
    );
}
