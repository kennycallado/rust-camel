// Goldens in tests/fixtures/golden were produced ONCE by protoc 31.1
// (protoc-bin-vendored 3.2.0):
//   PROTOC=~/.cargo/registry/src/*/protoc-bin-vendored-linux-x86_64-3.2.0/bin/protoc
//   $PROTOC --include_imports --descriptor_set_out=golden/kitchen.binpb -I ks/lib -I ks/main ks/main/kitchen.proto
//   $PROTOC --include_imports --descriptor_set_out=golden/ed2023.binpb -I ks/main ks/main/ed2023.proto
//   $PROTOC --include_imports --descriptor_set_out=golden/helloworld.binpb -I .. ../helloworld.proto
// Regenerate only when a fixture changes; never regenerate from protox output.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use camel_proto_compiler::compile_proto;
use prost::Message;
use prost_reflect::DescriptorPool;
use prost_reflect::DynamicMessage;
use prost_reflect::prost_types::{FileDescriptorProto, MethodOptions};

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(rel: &str) -> PathBuf {
    manifest().join("tests").join("fixtures").join(rel)
}

fn compile<P: AsRef<Path>>(path: P, includes: &[&Path]) -> DescriptorPool {
    compile_proto(path, includes).expect("proto compilation should succeed")
}

fn encode(pool: &DescriptorPool, message_name: &str, json: &str) -> Vec<u8> {
    let desc = pool
        .get_message_by_name(message_name)
        .unwrap_or_else(|| panic!("message not found: {message_name}"));
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let message = DynamicMessage::deserialize(desc, &mut deserializer)
        .expect("JSON should deserialize into the dynamic message");
    message.encode_to_vec()
}

/// Normalizes a pool for comparison against a protoc golden.
///
/// Two allowances are applied for known, benign producer differences:
/// - `google/protobuf/descriptor.proto` is skipped (protox serves an older
///   copy than protoc 31.1).
/// - source code info is dropped.
/// - an empty `MethodOptions` is removed (protoc emits the empty message,
///   protox omits it).
fn normalized(pool: &DescriptorPool) -> BTreeMap<String, FileDescriptorProto> {
    pool.file_descriptor_protos()
        .filter(|f| f.name.as_deref() != Some("google/protobuf/descriptor.proto"))
        .map(|f| {
            let mut file = f.clone();
            file.source_code_info = None;
            for service in &mut file.service {
                for method in &mut service.method {
                    if method.options.as_ref() == Some(&MethodOptions::default()) {
                        method.options = None;
                    }
                }
            }
            (file.name.clone().unwrap_or_default(), file)
        })
        .collect()
}

fn count_non_overlapping(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

#[test]
fn kitchen_sink_matches_protoc_golden() {
    let compiled = compile(fixture("ks/main/kitchen.proto"), &[&fixture("ks/lib")]);
    let golden = compile(fixture("golden/kitchen.binpb"), &[]);
    assert_eq!(normalized(&compiled), normalized(&golden));
}

#[test]
fn kitchen_sink_json_to_wire_matches_golden() {
    let compiled = compile(fixture("ks/main/kitchen.proto"), &[&fixture("ks/lib")]);
    let golden = compile(fixture("golden/kitchen.binpb"), &[]);

    let json = r#"{"id":"o1","note":"n","status":"ACTIVE","lines":[{"sku":"s","qty":2}],"weights":[1.5,2.5],"tags":{"7":"x"},"card":"4111","created":"2024-01-02T03:04:05Z","ttl":"3s","priority":"HIGH"}"#;

    let from_source = encode(&compiled, "kitchen.v1.Order", json);
    let from_golden = encode(&golden, "kitchen.v1.Order", json);
    assert_eq!(from_source, from_golden, "JSON wire encoding must match");

    let unpacked_tag = [0xa1, 0x01];
    let packed_tag = [0xa2, 0x01];
    assert!(
        count_non_overlapping(&from_source, &unpacked_tag) >= 2,
        "field 20 `weights` is packed=false and must emit two unpacked tags"
    );
    assert!(
        !from_source
            .windows(packed_tag.len())
            .any(|window| window == packed_tag),
        "field 20 must not use the packed wire tag"
    );
}

#[test]
fn proto3_packed_false_encodes_unpacked() {
    let pool = compile(fixture("packed/p3.proto"), &[]);
    let bytes = encode(&pool, "p3.P", r#"{"w":[1.0,2.0],"d":[3.0]}"#);
    assert_eq!(
        bytes,
        vec![
            0x09, 0, 0, 0, 0, 0, 0, 0xf0, 0x3f, // w[0] = 1.0, unpacked
            0x09, 0, 0, 0, 0, 0, 0, 0, 0x40, // w[1] = 2.0, unpacked
            0x12, 0x08, 0, 0, 0, 0, 0, 0, 0x08, 0x40, // d = [3.0], packed
        ]
    );
}

#[test]
fn proto2_packed_true_encodes_packed() {
    let pool = compile(fixture("packed/p2.proto"), &[]);
    let bytes = encode(&pool, "p2.P", r#"{"values":[1,2],"plain":[3,4]}"#);
    assert_eq!(
        bytes,
        vec![
            0x0a, 0x02, 0x01, 0x02, // values = [1, 2], packed
            0x10, 0x03, 0x10, 0x04, // plain = [3, 4], unpacked
        ]
    );
}

#[test]
fn repo_fixtures_compile() {
    let local_helloworld = compile(manifest().join("tests/helloworld.proto"), &[]);
    for name in ["helloworld.HelloRequest", "helloworld.HelloReply"] {
        assert!(
            local_helloworld.get_message_by_name(name).is_some(),
            "local helloworld must expose {name}"
        );
    }
    assert!(
        local_helloworld
            .get_service_by_name("helloworld.Greeter")
            .is_some()
    );

    let grpc_dir = manifest().join("../../components/camel-component-grpc/tests");
    let grpc_helloworld = compile(grpc_dir.join("helloworld.proto"), &[]);
    for name in ["helloworld.HelloRequest", "helloworld.HelloReply"] {
        assert!(
            grpc_helloworld.get_message_by_name(name).is_some(),
            "gRPC helloworld must expose {name}"
        );
    }
    assert!(
        grpc_helloworld
            .get_service_by_name("helloworld.Greeter")
            .is_some()
    );

    let streaming = compile(grpc_dir.join("streaming.proto"), &[]);
    for name in ["streaming.ListRequest", "streaming.EchoResponse"] {
        assert!(
            streaming.get_message_by_name(name).is_some(),
            "streaming must expose {name}"
        );
    }
    let stream_service = streaming
        .get_service_by_name("streaming.StreamService")
        .expect("streaming must expose streaming.StreamService");
    assert_eq!(stream_service.methods().count(), 3);

    let recursive = compile(
        manifest()
            .join("../../dataformats/camel-dataformat-protobuf/tests/fixtures/recursive.proto"),
        &[],
    );
    assert!(recursive.get_message_by_name("test.Node").is_some());
}

#[test]
fn repo_fixture_helloworld_json_roundtrip() {
    let path = manifest().join("../../components/camel-component-grpc/tests/helloworld.proto");
    let pool = compile(path, &[]);
    let bytes = encode(&pool, "helloworld.HelloRequest", r#"{"name":"Camel"}"#);
    assert_eq!(bytes, vec![0x0a, 0x05, b'C', b'a', b'm', b'e', b'l']);
}

#[test]
fn precompiled_helloworld_equals_compiled() {
    let compiled = compile(manifest().join("tests/helloworld.proto"), &[]);
    let golden = compile(fixture("golden/helloworld.binpb"), &[]);
    assert_eq!(normalized(&compiled), normalized(&golden));
}
