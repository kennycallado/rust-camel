//! Hostile-input integration tests: source nesting, imports and descriptor
//! sets. Every compilation that can recurse runs on a 2 MiB stack so a missing
//! guard aborts instead of passing unnoticed.

use std::path::{Path, PathBuf};

use camel_proto_compiler::{ProtoCache, ProtoCompileError, compile_proto};
use prost::Message;
use prost_reflect::DescriptorPool;
use prost_reflect::prost_types::{
    DescriptorProto, EnumDescriptorProto, EnumOptions, EnumValueDescriptorProto, EnumValueOptions,
    ExtensionRangeOptions, FieldDescriptorProto, FieldOptions, FileDescriptorProto,
    FileDescriptorSet, FileOptions, MessageOptions, MethodDescriptorProto, MethodOptions,
    OneofDescriptorProto, OneofOptions, ServiceDescriptorProto, ServiceOptions,
    UninterpretedOption,
    descriptor_proto::ExtensionRange,
    field_descriptor_proto::{Label, Type},
    uninterpreted_option::NamePart,
};

/// Runs `f` on a thread with a 2 MiB stack and joins it. A panic or abort in
/// `f` fails or kills the test.
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(f)
        .expect("spawn small-stack thread")
        .join()
        .expect("small-stack thread must not panic or abort")
}

/// Builds a proto3 source with `depth` nested messages, level `i` named `Mi`.
fn nested_messages(depth: usize) -> String {
    let mut text = String::from("syntax = \"proto3\";\n");
    for i in 0..depth {
        text.push_str(&format!("message M{i} {{ "));
    }
    text.push_str("string s = 1;");
    for _ in 0..depth {
        text.push('}');
    }
    text
}

/// Writes `text` to `dir/name` and returns the full path.
fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("write fixture");
    path
}

/// Asserts `r` is `ProtoCompileError::Compile` and returns its `detail`.
fn assert_compile_err(r: Result<DescriptorPool, ProtoCompileError>) -> String {
    match r {
        Err(ProtoCompileError::Compile { detail, .. }) => detail,
        other => panic!("expected Compile error, got {other:?}"),
    }
}

#[test]
fn message_nesting_100_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = write(dir.path(), "deep.proto", &nested_messages(100));
    let result = on_small_stack(move || compile_proto(&path, [dir.path()]));
    assert_compile_err(result);
}

#[test]
fn message_nesting_10000_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = write(dir.path(), "deep.proto", &nested_messages(10_000));
    let result = on_small_stack(move || compile_proto(&path, [dir.path()]));
    assert_compile_err(result);
}

#[test]
fn angle_bracket_option_nesting_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let text = "syntax = \"proto3\";\noption (r) = { f ".to_string()
        + &"< f ".repeat(10_000)
        + &"> ".repeat(10_000)
        + "};\n";
    let path = write(dir.path(), "angle.proto", &text);
    let result = on_small_stack(move || compile_proto(&path, [dir.path()]));
    let detail = assert_compile_err(result);
    assert!(detail.contains("nesting depth"), "detail: {detail}");
}

#[test]
fn square_bracket_nesting_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let text = "syntax = \"proto3\";\noption (r) = { f ".to_string()
        + &"[ f ".repeat(10_000)
        + &"] ".repeat(10_000)
        + "};\n";
    let path = write(dir.path(), "square.proto", &text);
    let result = on_small_stack(move || compile_proto(&path, [dir.path()]));
    let detail = assert_compile_err(result);
    assert!(detail.contains("nesting depth"), "detail: {detail}");
}

#[test]
fn unterminated_string_cannot_hide_nesting() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let mut text = String::from("syntax = \"proto3\";\noption x = \"abc\n");
    text.push_str(&"{".repeat(10_000));
    text.push('\n');
    let path = write(dir.path(), "unterminated.proto", &text);
    let result = on_small_stack(move || compile_proto(&path, [dir.path()]));
    let detail = assert_compile_err(result);
    assert!(detail.contains("nesting depth"), "detail: {detail}");
}

#[test]
fn escaped_quote_cannot_hide_nesting() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let mut text = String::from("syntax = \"proto3\";\noption x = \"\\\"\"");
    text.push(' ');
    text.push_str(&"{".repeat(10_000));
    text.push('\n');
    let path = write(dir.path(), "escaped.proto", &text);
    let result = on_small_stack(move || compile_proto(&path, [dir.path()]));
    let detail = assert_compile_err(result);
    assert!(detail.contains("nesting depth"), "detail: {detail}");
}

#[test]
fn deep_import_is_typed_error_and_names_the_import() {
    let dir = tempfile::tempdir().expect("tmp dir");
    write(
        dir.path(),
        "top.proto",
        "syntax = \"proto3\";\nimport \"deep.proto\";\nmessage T { string a = 1; }\n",
    );
    write(dir.path(), "deep.proto", &nested_messages(10_000));
    let top = dir.path().join("top.proto");
    let result = on_small_stack(move || compile_proto(&top, [dir.path()]));
    let detail = assert_compile_err(result);
    assert!(detail.contains("deep.proto"), "detail: {detail}");
    assert!(detail.contains("nesting depth"), "detail: {detail}");
}

#[test]
fn deep_nesting_inside_comment_is_accepted() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let mut text = String::from("syntax = \"proto3\";\n// ");
    text.push_str(&"{".repeat(10_000));
    text.push('\n');
    text.push_str("message Ok { string a = 1; }\n");
    let path = write(dir.path(), "comment.proto", &text);
    let result = on_small_stack(move || compile_proto(&path, [dir.path()]));
    let pool = result.expect("deep nesting inside a comment must be accepted");
    assert!(
        pool.get_message_by_name("Ok").is_some(),
        "pool must expose message Ok"
    );
}

#[test]
fn hostile_descriptor_set_is_typed_error() {
    for aggregate in [
        "f < ".repeat(10_000),
        "f { ".repeat(10_000),
        "f < # >\n".repeat(10_000),
    ] {
        let dir = tempfile::tempdir().expect("tmp dir");
        let path = dir.path().join("h.binpb");
        let set = FileDescriptorSet {
            file: vec![FileDescriptorProto {
                name: Some("h.proto".into()),
                options: Some(FileOptions {
                    uninterpreted_option: vec![UninterpretedOption {
                        name: vec![NamePart {
                            name_part: "x".into(),
                            is_extension: true,
                        }],
                        aggregate_value: Some(aggregate),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            }],
        };
        std::fs::write(&path, set.encode_to_vec()).expect("write hostile descriptor set");

        let result = on_small_stack(move || compile_proto(&path, std::iter::empty::<&Path>()));
        match result {
            Err(ProtoCompileError::DescriptorDecode(s)) => {
                assert!(s.contains("option text nesting depth"), "s: {s}")
            }
            other => panic!("expected DescriptorDecode error, got {other:?}"),
        }
    }
}

/// An `UninterpretedOption` whose aggregate text nests 100 `<` deep (> 64).
fn hostile_aggregate() -> UninterpretedOption {
    UninterpretedOption {
        name: vec![NamePart {
            name_part: "x".into(),
            is_extension: true,
        }],
        aggregate_value: Some("f < ".repeat(100)),
        ..Default::default()
    }
}

fn set_with_file(file: FileDescriptorProto) -> FileDescriptorSet {
    FileDescriptorSet { file: vec![file] }
}

fn set_with_message(message: DescriptorProto) -> FileDescriptorSet {
    set_with_file(FileDescriptorProto {
        name: Some("h.proto".into()),
        message_type: vec![message],
        ..Default::default()
    })
}

fn named_message(name: &str) -> DescriptorProto {
    DescriptorProto {
        name: Some(name.into()),
        ..Default::default()
    }
}

fn hostile_message_options() -> MessageOptions {
    MessageOptions {
        uninterpreted_option: vec![hostile_aggregate()],
        ..Default::default()
    }
}

fn hostile_field_options() -> FieldOptions {
    FieldOptions {
        uninterpreted_option: vec![hostile_aggregate()],
        ..Default::default()
    }
}

fn hostile_enum_options() -> EnumOptions {
    EnumOptions {
        uninterpreted_option: vec![hostile_aggregate()],
        ..Default::default()
    }
}

#[test]
fn hostile_aggregate_in_every_option_location() {
    let message_options = {
        let mut m = named_message("M");
        m.options = Some(hostile_message_options());
        set_with_message(m)
    };
    let nested_message_options = {
        let mut nested = named_message("N");
        nested.options = Some(hostile_message_options());
        let mut parent = named_message("M");
        parent.nested_type = vec![nested];
        set_with_message(parent)
    };
    let field_options = {
        let mut m = named_message("M");
        m.field = vec![FieldDescriptorProto {
            name: Some("f".into()),
            number: Some(1),
            options: Some(hostile_field_options()),
            ..Default::default()
        }];
        set_with_message(m)
    };
    let extension_field_options = {
        let mut m = named_message("M");
        m.extension = vec![FieldDescriptorProto {
            name: Some("e".into()),
            number: Some(100),
            options: Some(hostile_field_options()),
            ..Default::default()
        }];
        set_with_message(m)
    };
    let oneof_options = {
        let mut m = named_message("M");
        m.oneof_decl = vec![OneofDescriptorProto {
            name: Some("o".into()),
            options: Some(OneofOptions {
                uninterpreted_option: vec![hostile_aggregate()],
            }),
        }];
        set_with_message(m)
    };
    let extension_range_options = {
        let mut m = named_message("M");
        m.extension_range = vec![ExtensionRange {
            start: Some(100),
            end: Some(200),
            options: Some(ExtensionRangeOptions {
                uninterpreted_option: vec![hostile_aggregate()],
            }),
        }];
        set_with_message(m)
    };
    let enum_options = set_with_file(FileDescriptorProto {
        name: Some("h.proto".into()),
        enum_type: vec![EnumDescriptorProto {
            name: Some("E".into()),
            options: Some(hostile_enum_options()),
            ..Default::default()
        }],
        ..Default::default()
    });
    let nested_enum_options = {
        let mut m = named_message("M");
        m.enum_type = vec![EnumDescriptorProto {
            name: Some("E".into()),
            options: Some(hostile_enum_options()),
            ..Default::default()
        }];
        set_with_message(m)
    };
    let enum_value_options = set_with_file(FileDescriptorProto {
        name: Some("h.proto".into()),
        enum_type: vec![EnumDescriptorProto {
            name: Some("E".into()),
            value: vec![EnumValueDescriptorProto {
                name: Some("V".into()),
                number: Some(0),
                options: Some(EnumValueOptions {
                    uninterpreted_option: vec![hostile_aggregate()],
                    ..Default::default()
                }),
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    let service_options = set_with_file(FileDescriptorProto {
        name: Some("h.proto".into()),
        service: vec![ServiceDescriptorProto {
            name: Some("S".into()),
            options: Some(ServiceOptions {
                uninterpreted_option: vec![hostile_aggregate()],
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    });
    let method_options = set_with_file(FileDescriptorProto {
        name: Some("h.proto".into()),
        service: vec![ServiceDescriptorProto {
            name: Some("S".into()),
            method: vec![MethodDescriptorProto {
                name: Some("M".into()),
                options: Some(MethodOptions {
                    uninterpreted_option: vec![hostile_aggregate()],
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    let file_extension_options = set_with_file(FileDescriptorProto {
        name: Some("h.proto".into()),
        extension: vec![FieldDescriptorProto {
            name: Some("e".into()),
            number: Some(100),
            options: Some(hostile_field_options()),
            ..Default::default()
        }],
        ..Default::default()
    });

    let cases: Vec<(&str, FileDescriptorSet)> = vec![
        ("message options", message_options),
        ("nested message options", nested_message_options),
        ("field options", field_options),
        ("extension field options", extension_field_options),
        ("oneof options", oneof_options),
        ("extension range options", extension_range_options),
        ("enum options", enum_options),
        ("nested enum options", nested_enum_options),
        ("enum value options", enum_value_options),
        ("service options", service_options),
        ("method options", method_options),
        ("file-level extension options", file_extension_options),
    ];

    for (label, set) in cases {
        let dir = tempfile::tempdir().expect("tmp dir");
        let path = dir.path().join("h.binpb");
        std::fs::write(&path, set.encode_to_vec()).expect("write hostile descriptor set");
        let result = on_small_stack(move || compile_proto(&path, std::iter::empty::<&Path>()));
        match result {
            Err(ProtoCompileError::DescriptorDecode(s)) => {
                assert!(
                    s.contains("option text nesting depth"),
                    "location {label}: expected nesting error, got {s}"
                )
            }
            other => panic!("location {label}: expected DescriptorDecode error, got {other:?}"),
        }
    }
}

/// Builds a `len`-file import chain in `dir`: `f0.proto` imports `f1.proto`,
/// ..., `f{len-2}.proto` imports `f{len-1}.proto`, and the last file declares
/// `chain.End`. Every file declares `package chain;`.
fn write_chain(dir: &Path, len: usize) -> PathBuf {
    for i in 0..len {
        let mut text = String::from("syntax = \"proto3\";\npackage chain;\n");
        if i + 1 < len {
            text.push_str(&format!("import \"f{}.proto\";\n", i + 1));
        } else {
            text.push_str("message End { string a = 1; }\n");
        }
        std::fs::write(dir.join(format!("f{i}.proto")), text).expect("write chain file");
    }
    dir.join("f0.proto")
}

#[test]
fn import_chain_over_limit_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let first = write_chain(dir.path(), 300);
    let result = on_small_stack(move || compile_proto(&first, [dir.path()]));
    let detail = assert_compile_err(result);
    assert!(detail.contains("import graph exceeds"), "detail: {detail}");
}

#[test]
fn import_chain_under_limit_compiles() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let first = write_chain(dir.path(), 200);
    let pool = compile_proto(&first, [dir.path()]).expect("200-file chain must compile");
    assert!(
        pool.get_message_by_name("chain.End").is_some(),
        "pool must expose chain.End"
    );
}

/// A single-file descriptor with one public dependency on `dep`.
fn descriptor_file(name: &str, dep: &str) -> FileDescriptorProto {
    FileDescriptorProto {
        name: Some(name.into()),
        syntax: Some("proto3".into()),
        dependency: vec![dep.into()],
        public_dependency: vec![0],
        ..Default::default()
    }
}

#[test]
fn descriptor_self_import_cycle_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("x.binpb");
    let set = FileDescriptorSet {
        file: vec![descriptor_file("x.proto", "x.proto")],
    };
    std::fs::write(&path, set.encode_to_vec()).expect("write descriptor set");

    let result = on_small_stack(move || compile_proto(&path, std::iter::empty::<&Path>()));
    match result {
        Err(ProtoCompileError::DescriptorDecode(s)) => {
            assert!(s.contains("cyclic import graph"), "s: {s}")
        }
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

#[test]
fn two_file_public_import_cycle_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("cycle.binpb");
    let set = FileDescriptorSet {
        file: vec![
            descriptor_file("a.proto", "b.proto"),
            descriptor_file("b.proto", "a.proto"),
        ],
    };
    std::fs::write(&path, set.encode_to_vec()).expect("write descriptor set");

    let result = on_small_stack(move || compile_proto(&path, std::iter::empty::<&Path>()));
    match result {
        Err(ProtoCompileError::DescriptorDecode(s)) => {
            assert!(s.contains("cyclic import graph"), "s: {s}");
            assert!(s.contains("a.proto"), "s: {s}");
            assert!(s.contains("b.proto"), "s: {s}");
        }
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

/// Creates a sparse file of `len` bytes without writing the data.
fn sparse_file(path: &Path, len: u64) {
    let file = std::fs::File::create(path).expect("create sparse file");
    file.set_len(len).expect("set_len");
}

#[test]
fn oversized_source_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let big = dir.path().join("big.proto");
    sparse_file(&big, 2 << 30);

    let err = compile_proto(&big, std::iter::empty::<&Path>()).expect_err("oversized must fail");
    assert!(format!("{err}").contains("exceeds the limit"), "err: {err}");

    let cache = ProtoCache::new();
    let err = cache
        .get_or_compile(&big, std::iter::empty::<&Path>())
        .expect_err("cache hash path must fail");
    assert!(format!("{err}").contains("exceeds the limit"), "err: {err}");
}

#[test]
fn oversized_descriptor_set_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let big = dir.path().join("big.binpb");
    sparse_file(&big, 2 << 30);

    let err = compile_proto(&big, std::iter::empty::<&Path>()).expect_err("oversized must fail");
    match err {
        ProtoCompileError::DescriptorDecode(s) => {
            assert!(s.contains("exceeds the limit"), "s: {s}")
        }
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

/// A proto3 descriptor file that public-imports every name in `deps`.
fn public_deps_file(name: &str, deps: &[&str]) -> FileDescriptorProto {
    FileDescriptorProto {
        name: Some(name.into()),
        syntax: Some("proto3".into()),
        dependency: deps.iter().map(|d| (*d).into()).collect(),
        public_dependency: (0..deps.len() as i32).collect(),
        ..Default::default()
    }
}

/// A chain of `len` files: `c0` is the leaf, `c_i` public-imports `c_{i-1}`.
fn public_chain_files(len: usize) -> Vec<FileDescriptorProto> {
    (0..len)
        .map(|i| {
            if i == 0 {
                public_deps_file("c0.proto", &[])
            } else {
                let dep = format!("c{}.proto", i - 1);
                public_deps_file(&format!("c{i}.proto"), &[dep.as_str()])
            }
        })
        .collect()
}

/// `roots` files that private-import all `chain` names leaf-first.
fn private_root_files(roots: usize, chain: usize) -> Vec<FileDescriptorProto> {
    let names: Vec<String> = (0..chain).map(|i| format!("c{i}.proto")).collect();
    (0..roots)
        .map(|r| FileDescriptorProto {
            name: Some(format!("r{r}.proto")),
            syntax: Some("proto3".into()),
            dependency: names.clone(),
            ..Default::default()
        })
        .collect()
}

#[test]
fn private_import_roots_descriptor_set_is_typed_error() {
    // 150 public-chain files plus 1000 private roots, each private-importing
    // all 150 chain names; W ~11.3M exceeds the resolution budget.
    let mut files = public_chain_files(150);
    files.extend(private_root_files(1000, 150));
    let set = FileDescriptorSet { file: files };
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("work.binpb");
    std::fs::write(&path, set.encode_to_vec()).expect("write work descriptor set");

    let result = on_small_stack(move || compile_proto(&path, std::iter::empty::<&Path>()));
    match result {
        Err(ProtoCompileError::DescriptorDecode(s)) => {
            assert!(s.contains("resolution budget"), "s: {s}")
        }
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

/// The 1000-file leaf-first pattern: `f_i` public-imports `f_{i-1}` and a
/// root public-imports `f999..f500`, so the longest chain is 1001 > 256.
fn leaf_first_set() -> FileDescriptorSet {
    let mut files = vec![public_deps_file("f0.proto", &[])];
    for i in 1..1000 {
        let dep = format!("f{}.proto", i - 1);
        files.push(public_deps_file(&format!("f{i}.proto"), &[dep.as_str()]));
    }
    let roots: Vec<String> = (500..1000).rev().map(|i| format!("f{i}.proto")).collect();
    let root_refs: Vec<&str> = roots.iter().map(String::as_str).collect();
    files.push(public_deps_file("root.proto", &root_refs));
    FileDescriptorSet { file: files }
}

#[test]
fn leaf_first_descriptor_set_is_typed_error() {
    let set = leaf_first_set();
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("leaf_first.binpb");
    std::fs::write(&path, set.encode_to_vec()).expect("write leaf-first descriptor set");

    let result = on_small_stack(move || compile_proto(&path, std::iter::empty::<&Path>()));
    match result {
        Err(ProtoCompileError::DescriptorDecode(s)) => {
            assert!(s.contains("import chain exceeds"), "s: {s}")
        }
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

#[test]
fn acyclic_layered_descriptor_set_is_typed_error() {
    const LAYERS: usize = 7;
    const WIDTH: usize = 10;
    let name = |layer: usize, k: usize| format!("l{layer}_{k}.proto");
    let mut files: Vec<FileDescriptorProto> = (0..WIDTH)
        .map(|k| public_deps_file(&name(0, k), &[]))
        .collect();
    for layer in 1..LAYERS {
        let prev: Vec<String> = (0..WIDTH).map(|k| name(layer - 1, k)).collect();
        let prev_refs: Vec<&str> = prev.iter().map(String::as_str).collect();
        for k in 0..WIDTH {
            files.push(public_deps_file(&name(layer, k), &prev_refs));
        }
    }
    let set = FileDescriptorSet { file: files };
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("layered.binpb");
    std::fs::write(&path, set.encode_to_vec()).expect("write descriptor set");

    let result = on_small_stack(move || compile_proto(&path, std::iter::empty::<&Path>()));
    match result {
        Err(ProtoCompileError::DescriptorDecode(s)) => {
            assert!(s.contains("resolution budget"), "s: {s}")
        }
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

#[test]
fn acyclic_chain_descriptor_set_is_typed_error() {
    let mut files = vec![public_deps_file("f0.proto", &[])];
    for i in 1..300 {
        let dep = format!("f{}.proto", i - 1);
        files.push(public_deps_file(&format!("f{i}.proto"), &[dep.as_str()]));
    }
    let set = FileDescriptorSet { file: files };
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("chain300.binpb");
    std::fs::write(&path, set.encode_to_vec()).expect("write descriptor set");

    let result = on_small_stack(move || compile_proto(&path, std::iter::empty::<&Path>()));
    match result {
        Err(ProtoCompileError::DescriptorDecode(s)) => {
            assert!(s.contains("import chain exceeds"), "s: {s}")
        }
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

#[test]
fn acyclic_descriptor_graph_under_bounds_loads() {
    let leaf = FileDescriptorProto {
        name: Some("leaf.proto".into()),
        package: Some("diamond".into()),
        syntax: Some("proto3".into()),
        message_type: vec![DescriptorProto {
            name: Some("Leaf".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let a = {
        let mut f = public_deps_file("a.proto", &["leaf.proto"]);
        f.package = Some("diamond".into());
        f.message_type = vec![DescriptorProto {
            name: Some("A".into()),
            ..Default::default()
        }];
        f
    };
    let b = {
        let mut f = public_deps_file("b.proto", &["leaf.proto"]);
        f.package = Some("diamond".into());
        f.message_type = vec![DescriptorProto {
            name: Some("B".into()),
            ..Default::default()
        }];
        f
    };
    let root = {
        let mut f = public_deps_file("root.proto", &["a.proto", "b.proto"]);
        f.package = Some("diamond".into());
        f.message_type = vec![DescriptorProto {
            name: Some("Root".into()),
            field: vec![FieldDescriptorProto {
                name: Some("l".into()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".diamond.Leaf".into()),
                ..Default::default()
            }],
            ..Default::default()
        }];
        f
    };
    let set = FileDescriptorSet {
        file: vec![leaf, a, b, root],
    };
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("diamond.binpb");
    std::fs::write(&path, set.encode_to_vec()).expect("write descriptor set");

    let result = on_small_stack(move || compile_proto(&path, std::iter::empty::<&Path>()));
    let pool = result.expect("under-bounds descriptor graph must load");
    assert!(
        pool.get_message_by_name("diamond.Leaf").is_some(),
        "pool must expose diamond.Leaf through the public imports"
    );
}

/// Writes `layers` layers of `width` `L{l}_{k}.proto` files plus a private
/// `root.proto` that imports every layer file. Every file of layer `l > 0`
/// public-imports every file of layer `l - 1`. Returns the root path.
fn write_layered_public_sources(dir: &Path, layers: usize, width: usize) -> PathBuf {
    let name = |l: usize, k: usize| format!("L{l}_{k}.proto");
    for k in 0..width {
        std::fs::write(
            dir.join(name(0, k)),
            format!("syntax = \"proto3\";\nmessage M0_{k} {{ string a = 1; }}\n"),
        )
        .expect("write layer 0 file");
    }
    for l in 1..layers {
        let mut imports = String::from("syntax = \"proto3\";\n");
        for pk in 0..width {
            imports.push_str(&format!("import public \"{}\";\n", name(l - 1, pk)));
        }
        for k in 0..width {
            let text = format!("{imports}message M{l}_{k} {{ string a = 1; }}\n");
            std::fs::write(dir.join(name(l, k)), text).expect("write layer file");
        }
    }
    let mut root = String::from("syntax = \"proto3\";\n");
    for l in 0..layers {
        for k in 0..width {
            root.push_str(&format!("import \"{}\";\n", name(l, k)));
        }
    }
    root.push_str("message Root { string a = 1; }\n");
    write(dir, "root.proto", &root)
}

#[test]
fn layered_public_import_source_graph_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let root = write_layered_public_sources(dir.path(), 70, 2);
    let result = on_small_stack(move || compile_proto(&root, [dir.path()]));
    let detail = assert_compile_err(result);
    assert!(detail.contains("resolution budget"), "detail: {detail}");
}

#[test]
fn source_lifetime_budget_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let root = write_layered_public_sources(dir.path(), 10, 2);
    let result = on_small_stack(move || compile_proto(&root, [dir.path()]));
    let detail = assert_compile_err(result);
    assert!(detail.contains("lifetime"), "detail: {detail}");
}
