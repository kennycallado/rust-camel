//! Unit tests for `compiler.rs`, moved out of it into this test-only module.

// The whole file is test code; this marker also tells the source scan in
// tests/hermetic.rs where test code begins.
#[cfg(test)]
use std::path::Path;

use prost_reflect::DescriptorPool;
use prost_reflect::prost_types::{
    FileDescriptorProto, FileOptions, uninterpreted_option::NamePart,
};

use super::*;

#[test]
fn contained_compile_maps_panic() {
    let result = contained_compile(Path::new("x.proto"), || panic!("boom"));
    match result {
        Err(ProtoCompileError::Compile { detail, .. }) => {
            assert!(
                detail.starts_with("internal compiler panic:"),
                "detail: {detail}"
            );
            assert!(detail.contains("boom"), "detail: {detail}");
        }
        other => panic!("expected Compile error, got {other:?}"),
    }
}

#[test]
fn contained_compile_passes_ok_and_err_through() {
    let path = Path::new("x.proto");
    assert!(contained_compile(path, || Ok(DescriptorPool::new())).is_ok());

    let result = contained_compile(path, || {
        Err(ProtoCompileError::DescriptorDecode("e".into()))
    });
    match result {
        Err(ProtoCompileError::DescriptorDecode(s)) => assert_eq!(s, "e"),
        other => panic!("expected DescriptorDecode, got {other:?}"),
    }
}

#[test]
fn edition_detail_gets_remedy() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("ed.proto");
    std::fs::write(
        &path,
        "edition = \"2023\";\npackage ed;\nmessage E { string a = 1; }\n",
    )
    .expect("write editions proto");

    let err = compile_proto(&path, std::iter::once(dir.path())).expect_err("editions must fail");
    match err {
        ProtoCompileError::Compile { detail, .. } => {
            assert!(detail.contains("editions"), "detail: {detail}");
            assert!(detail.contains("proto3"), "detail: {detail}");
        }
        other => panic!("expected Compile error, got {other:?}"),
    }
}

#[test]
fn contained_decode_maps_panic() {
    let result = contained_decode(Path::new("x.binpb"), || panic!("boom"));
    match result {
        Err(ProtoCompileError::DescriptorDecode(s)) => {
            assert!(s.starts_with("internal decoder panic:"), "s: {s}");
            assert!(s.contains("boom"), "s: {s}");
            assert!(s.contains("x.binpb"), "s: {s}");
        }
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

#[test]
fn is_descriptor_set_extensions() {
    assert!(is_descriptor_set(Path::new("a.binpb")));
    assert!(is_descriptor_set(Path::new("a.PB")));
    assert!(is_descriptor_set(Path::new("a.Desc")));
    assert!(is_descriptor_set(Path::new("a.protoset")));
    assert!(!is_descriptor_set(Path::new("a.proto")));
    assert!(!is_descriptor_set(Path::new("a")));
    assert!(!is_descriptor_set(Path::new("a.binpbx")));
}

#[test]
fn descriptor_set_loads_via_compile_proto() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("helloworld.desc");
    let pool =
        compile_proto(&path, std::iter::empty::<&Path>()).expect("descriptor set should load");
    assert!(
        pool.get_message_by_name("helloworld.HelloRequest")
            .is_some(),
        "descriptor set must expose helloworld.HelloRequest"
    );
}

#[test]
fn handbuilt_editions_descriptor_set_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("e.binpb");
    let set = FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some("e.proto".into()),
            syntax: Some("editions".into()),
            ..Default::default()
        }],
    };
    std::fs::write(&path, set.encode_to_vec()).expect("write descriptor set");

    let err = compile_proto(&path, std::iter::empty::<&Path>()).expect_err("must fail");
    match err {
        ProtoCompileError::DescriptorDecode(s) => assert!(s.contains("editions"), "s: {s}"),
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

#[test]
fn corrupt_descriptor_set_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("bad.binpb");
    std::fs::write(&path, [0xff, 0xff, 0xff, 0xff, 0xff]).expect("write corrupt set");

    let err = compile_proto(&path, std::iter::empty::<&Path>()).expect_err("must fail");
    match err {
        ProtoCompileError::DescriptorDecode(s) => assert!(s.contains("bad.binpb"), "s: {s}"),
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

fn wrapper(dir: &Path) -> NestingGuardedInclude {
    NestingGuardedInclude::new(
        dir.to_path_buf(),
        Arc::new(ImportBudget::default()),
        Arc::new(Mutex::new(None)),
    )
}

#[test]
fn invalid_utf8_include_falls_through_to_inner_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("bad.proto");
    let mut bytes = vec![0xff];
    bytes.extend_from_slice(&[b'{'; 70]);
    std::fs::write(&path, &bytes).expect("write invalid UTF-8 proto");

    let include = wrapper(dir.path());
    let err = match include.open_file("bad.proto") {
        Ok(_) => panic!("invalid UTF-8 must not compile"),
        Err(e) => e,
    };
    let rendered = format!("{err:?} {err}");
    assert!(
        !rendered.contains("nesting depth"),
        "invalid UTF-8 must not be classified as a nesting error: {rendered}"
    );
}

#[test]
fn resolver_reads_and_parses_same_buffer() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let text = "syntax = \"proto3\";\nmessage Ok { string a = 1; }\n";
    std::fs::write(dir.path().join("ok.proto"), text).expect("write proto");

    let file = wrapper(dir.path())
        .open_file("ok.proto")
        .expect("open ok.proto");
    assert_eq!(file.source(), Some(text));
    assert_eq!(
        file.file_descriptor_proto().name.as_deref(),
        Some("ok.proto")
    );
}

#[test]
fn resolver_rejects_directory_include() {
    let dir = tempfile::tempdir().expect("tmp dir");
    std::fs::create_dir(dir.path().join("x.proto")).expect("mkdir");

    let err = wrapper(dir.path())
        .open_file("x.proto")
        .expect_err("directory must fail");
    assert!(
        format!("{err}").contains("not a regular file"),
        "err: {err}"
    );
}

#[test]
fn resolver_shadow_check_replicates_protox() {
    let dir = tempfile::tempdir().expect("tmp dir");
    std::fs::create_dir(dir.path().join("a")).expect("mkdir a");
    std::fs::create_dir(dir.path().join("b")).expect("mkdir b");
    let a_proto = dir.path().join("a").join("dup.proto");
    let b_proto = dir.path().join("b").join("dup.proto");
    std::fs::write(
        &a_proto,
        "syntax = \"proto3\";\nmessage A { string a = 1; }\n",
    )
    .expect("write a");
    std::fs::write(
        &b_proto,
        "syntax = \"proto3\";\nmessage B { string b = 1; }\n",
    )
    .expect("write b");

    let top = Arc::new(Mutex::new(None));
    let include_a = NestingGuardedInclude::new(
        dir.path().join("a"),
        Arc::new(ImportBudget::default()),
        Arc::clone(&top),
    );
    let include_b = NestingGuardedInclude::new(
        dir.path().join("b"),
        Arc::new(ImportBudget::default()),
        Arc::clone(&top),
    );

    assert_eq!(
        include_b.resolve_path(&b_proto),
        Some("dup.proto".to_owned())
    );
    let err = include_a
        .open_file("dup.proto")
        .expect_err("shadowed input must fail");
    let rendered = format!("{err}");
    assert!(rendered.contains("shadowed by"), "err: {rendered}");
    assert!(
        rendered.contains(&a_proto.display().to_string()),
        "err: {rendered}"
    );
}

#[test]
fn import_budget_records_and_rejects() {
    let budget = ImportBudget::default();
    for i in 0..MAX_IMPORT_FILES {
        budget
            .record(&format!("f{i}.proto"), 1)
            .expect("under limit");
    }
    let err = budget
        .record("over.proto", 1)
        .expect_err("over limit must fail");
    assert!(
        format!("{err}").contains("import graph exceeds"),
        "err: {err}"
    );
}

#[test]
fn cycle_check_detects_self_and_pair() {
    let one = |name: &str, dep: &str| FileDescriptorProto {
        name: Some(name.to_owned()),
        syntax: Some("proto3".to_owned()),
        dependency: vec![dep.to_owned()],
        public_dependency: vec![0],
        ..Default::default()
    };

    let self_set = FileDescriptorSet {
        file: vec![one("x.proto", "x.proto")],
    };
    let err = check_import_cycles(&self_set)
        .map_err(|cycle| format!("cyclic import graph: {cycle}"))
        .expect_err("self import must be rejected");
    assert!(err.contains("cyclic import graph"), "err: {err}");
    assert!(err.contains("x.proto"), "err: {err}");

    let pair = FileDescriptorSet {
        file: vec![one("a.proto", "b.proto"), one("b.proto", "a.proto")],
    };
    let err = check_import_cycles(&pair)
        .map_err(|cycle| format!("cyclic import graph: {cycle}"))
        .expect_err("pair cycle must be rejected");
    assert!(err.contains("cyclic import graph"), "err: {err}");
    assert!(err.contains("a.proto"), "err: {err}");
    assert!(err.contains("b.proto"), "err: {err}");

    let acyclic = FileDescriptorSet {
        file: vec![
            FileDescriptorProto {
                name: Some("leaf.proto".to_owned()),
                syntax: Some("proto3".to_owned()),
                ..Default::default()
            },
            FileDescriptorProto {
                name: Some("left.proto".to_owned()),
                syntax: Some("proto3".to_owned()),
                dependency: vec!["leaf.proto".to_owned()],
                ..Default::default()
            },
            FileDescriptorProto {
                name: Some("right.proto".to_owned()),
                syntax: Some("proto3".to_owned()),
                dependency: vec!["leaf.proto".to_owned()],
                ..Default::default()
            },
            FileDescriptorProto {
                name: Some("top.proto".to_owned()),
                syntax: Some("proto3".to_owned()),
                dependency: vec!["left.proto".to_owned(), "right.proto".to_owned()],
                ..Default::default()
            },
            FileDescriptorProto {
                name: Some("chain.proto".to_owned()),
                syntax: Some("proto3".to_owned()),
                dependency: vec!["top.proto".to_owned()],
                ..Default::default()
            },
        ],
    };
    assert!(check_import_cycles(&acyclic).is_ok());
}

#[test]
fn oversized_include_is_rejected_before_read() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("big.proto");
    let file = std::fs::File::create(&path).expect("create");
    file.set_len(2 << 30).expect("set_len");
    drop(file);

    let err = wrapper(dir.path())
        .open_file("big.proto")
        .expect_err("oversized must fail");
    assert!(format!("{err}").contains("exceeds the limit"), "err: {err}");
}

#[test]
fn hostile_option_text_is_typed_error() {
    for aggregate in ["f < ".repeat(10_000), "f < # >\n".repeat(10_000)] {
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
        std::fs::write(&path, set.encode_to_vec()).expect("write hostile set");

        let thread_path = path.clone();
        let handle = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || compile_proto(&thread_path, std::iter::empty::<&Path>()))
            .expect("spawn thread");
        let err = handle
            .join()
            .expect("thread must not abort")
            .expect_err("must fail");
        match err {
            ProtoCompileError::DescriptorDecode(s) => {
                assert!(s.contains("option text nesting depth"), "s: {s}")
            }
            other => panic!("expected DescriptorDecode error, got {other:?}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn fifo_include_is_typed_error_without_hanging() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let fifo = dir.path().join("pipe.proto");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo failed: {status}");
    std::fs::write(
        dir.path().join("main.proto"),
        "syntax = \"proto3\";\nimport \"pipe.proto\";\nmessage M { string a = 1; }\n",
    )
    .expect("write main proto");

    // Returning at all proves the FIFO open did not block without a writer.
    let err = compile_proto(dir.path().join("main.proto"), std::iter::once(dir.path()))
        .expect_err("fifo import must fail");
    match err {
        ProtoCompileError::Compile { detail, .. } => {
            assert!(detail.contains("not a regular file"), "detail: {detail}")
        }
        other => panic!("expected Compile error, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn fifo_descriptor_set_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let fifo = dir.path().join("pipe.binpb");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo failed: {status}");

    let err = compile_proto(&fifo, std::iter::empty::<&Path>()).expect_err("fifo must fail");
    match err {
        ProtoCompileError::DescriptorDecode(s) => {
            assert!(s.contains("not a regular file"), "s: {s}")
        }
        other => panic!("expected DescriptorDecode error, got {other:?}"),
    }
}

#[test]
fn directory_source_is_typed_error() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("thing.proto");
    std::fs::create_dir(&path).expect("mkdir thing.proto");

    let err = compile_proto(&path, std::iter::empty::<&Path>()).expect_err("directory must fail");
    let rendered = match err {
        ProtoCompileError::Compile { detail, .. } => detail,
        ProtoCompileError::DescriptorDecode(s) => s,
        other => panic!("expected a typed schema error, got {other:?}"),
    };
    assert!(
        rendered.contains("not a regular file"),
        "directory must be rejected by metadata, not the OS: {rendered}"
    );
}

/// A proto3 descriptor file that public-imports every name in `deps`.
fn public_deps_file(name: &str, deps: &[&str]) -> FileDescriptorProto {
    FileDescriptorProto {
        name: Some(name.to_owned()),
        syntax: Some("proto3".to_owned()),
        dependency: deps.iter().map(|d| (*d).to_owned()).collect(),
        public_dependency: (0..deps.len() as i32).collect(),
        ..Default::default()
    }
}

#[test]
fn graph_chain_over_bound_rejected() {
    // f_i public-depends on f_{i-1}; the longest chain is 300 > 256.
    let mut files = vec![public_deps_file("f0.proto", &[])];
    for i in 1..300 {
        let dep = format!("f{}.proto", i - 1);
        files.push(public_deps_file(&format!("f{i}.proto"), &[dep.as_str()]));
    }
    let set = FileDescriptorSet { file: files };
    let err = check_descriptor_graph(&set).expect_err("300-chain must be rejected");
    assert!(err.contains("import chain exceeds"), "err: {err}");
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
            syntax: Some("proto3".to_owned()),
            dependency: names.clone(),
            ..Default::default()
        })
        .collect()
}

#[test]
fn private_import_roots_exceed_work_budget() {
    // 150 public-chain files plus 1000 private roots, each private-importing
    // all 150 chain names. Every private entry point starts a public-import
    // walk, so W is ~11.3M. The old budget (sum of E over files) is only
    // ~12.3k and would have accepted this set.
    let mut files = public_chain_files(150);
    files.extend(private_root_files(1000, 150));

    let forward = FileDescriptorSet {
        file: files.clone(),
    };
    let err = check_descriptor_graph(&forward).expect_err("work budget must reject forward order");
    assert!(err.contains("resolution budget"), "err: {err}");

    files.reverse();
    let reversed = FileDescriptorSet { file: files };
    let err =
        check_descriptor_graph(&reversed).expect_err("work budget must reject reversed order");
    assert!(err.contains("resolution budget"), "err: {err}");
}

#[test]
fn graph_layered_dag_rejected_by_expansion() {
    // 7 layers x 10 files; each file public-depends on every file of the
    // previous layer. Chain length 7 <= 256 but expansion ~1.2e6.
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
    let err = check_descriptor_graph(&set).expect_err("layered DAG must be rejected");
    assert!(err.contains("resolution budget"), "err: {err}");
}

#[test]
fn graph_leaf_first_revisit_rejected() {
    // f_i public-depends on f_{i-1} for i in 1..1000; root depends on
    // f_999..f_500. The longest chain is 1001 > 256.
    let mut files = vec![public_deps_file("f0.proto", &[])];
    for i in 1..1000 {
        let dep = format!("f{}.proto", i - 1);
        files.push(public_deps_file(&format!("f{i}.proto"), &[dep.as_str()]));
    }
    let roots: Vec<String> = (500..1000).rev().map(|i| format!("f{i}.proto")).collect();
    let root_refs: Vec<&str> = roots.iter().map(String::as_str).collect();
    files.push(public_deps_file("root.proto", &root_refs));
    let set = FileDescriptorSet { file: files };
    let err = check_descriptor_graph(&set).expect_err("deep chain must be rejected");
    assert!(err.contains("import chain exceeds"), "err: {err}");
}

#[test]
fn graph_small_diamond_ok() {
    // root -> a, b; a, b -> leaf (all public); leaf has a message and root
    // has a field of the leaf type.
    let leaf = {
        let mut f = public_deps_file("leaf.proto", &[]);
        f.package = Some("diamond".to_owned());
        f.message_type = vec![DescriptorProto {
            name: Some("Leaf".to_owned()),
            ..Default::default()
        }];
        f
    };
    let a = {
        let mut f = public_deps_file("a.proto", &["leaf.proto"]);
        f.package = Some("diamond".to_owned());
        f
    };
    let b = {
        let mut f = public_deps_file("b.proto", &["leaf.proto"]);
        f.package = Some("diamond".to_owned());
        f
    };
    let root = {
        let mut f = public_deps_file("root.proto", &["a.proto", "b.proto"]);
        f.package = Some("diamond".to_owned());
        f.message_type = vec![DescriptorProto {
            name: Some("Root".to_owned()),
            field: vec![prost_reflect::prost_types::FieldDescriptorProto {
                name: Some("l".to_owned()),
                number: Some(1),
                type_name: Some(".diamond.Leaf".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        }];
        f
    };
    let set = FileDescriptorSet {
        file: vec![leaf, a, b, root],
    };
    assert!(check_descriptor_graph(&set).is_ok());
}

/// `layers` layers of `width` files: every file of layer `l > 0`
/// public-imports every file of layer `l - 1`; a private `root.proto` imports
/// every layer file. Names are `L{l}_{k}.proto`, the root is `root.proto`.
fn layered_source_set(layers: usize, width: usize) -> FileDescriptorSet {
    let name = |l: usize, k: usize| format!("L{l}_{k}.proto");
    let mut files: Vec<FileDescriptorProto> = (0..width)
        .map(|k| public_deps_file(&name(0, k), &[]))
        .collect();
    for l in 1..layers {
        let prev: Vec<String> = (0..width).map(|k| name(l - 1, k)).collect();
        let prev_refs: Vec<&str> = prev.iter().map(String::as_str).collect();
        for k in 0..width {
            files.push(public_deps_file(&name(l, k), &prev_refs));
        }
    }
    let mut all: Vec<String> = Vec::with_capacity(layers * width);
    for l in 0..layers {
        for k in 0..width {
            all.push(name(l, k));
        }
    }
    files.push(FileDescriptorProto {
        name: Some("root.proto".into()),
        syntax: Some("proto3".into()),
        dependency: all,
        ..Default::default()
    });
    FileDescriptorSet { file: files }
}

#[test]
fn layered_source_graph_budget_helper_is_typed_error() {
    // 70 width-two public layers + root: 141 files, chain 71 <= 256 but W is
    // astronomical, so the resolution budget rejects before Compiler exists.
    let set = layered_source_set(70, 2);
    let err = check_source_graph(Path::new("root.proto"), &set)
        .expect_err("layered source graph must exceed the resolution budget");
    match err {
        ProtoCompileError::Compile { detail, .. } => {
            assert!(detail.contains("resolution budget"), "detail: {detail}");
        }
        other => panic!("expected Compile error, got {other:?}"),
    }
}

#[test]
fn source_lifetime_budget_is_typed_error() {
    // 10 width-two layers + root: W = 8145 is under the graph budget, but
    // (file_count + 1) * W = 22 * 8145 = 179_190 exceeds the 100_000 lifetime.
    let set = layered_source_set(10, 2);
    assert!(
        check_descriptor_graph(&set).is_ok(),
        "10 layers must fit the descriptor graph budget on their own"
    );
    let err = check_source_graph(Path::new("root.proto"), &set)
        .expect_err("source lifetime must exceed the budget");
    match err {
        ProtoCompileError::Compile { detail, .. } => {
            assert!(detail.contains("lifetime"), "detail: {detail}");
        }
        other => panic!("expected Compile error, got {other:?}"),
    }
}

#[test]
fn missing_import_detail_keeps_source_position() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("missing.proto");
    std::fs::write(
        &path,
        "syntax = \"proto3\";\nimport \"nope/missing.proto\";\nmessage M { string x = 1; }\n",
    )
    .expect("write proto");

    let err =
        compile_proto(&path, std::iter::empty::<&Path>()).expect_err("missing import must fail");
    match err {
        ProtoCompileError::Compile { detail, .. } => {
            assert!(detail.contains("missing.proto:2:1"), "detail: {detail}");
            assert!(detail.contains("nope/missing.proto"), "detail: {detail}");
        }
        other => panic!("expected Compile error, got {other:?}"),
    }
}

#[test]
fn source_snapshot_survives_file_replacement() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let main = dir.path().join("main.proto");
    let dep = dir.path().join("dep.proto");
    std::fs::write(
        &main,
        "syntax = \"proto3\";\npackage snap;\nimport \"dep.proto\";\nmessage A { B b = 1; }\n",
    )
    .expect("write main");
    std::fs::write(
        &dep,
        "syntax = \"proto3\";\npackage snap;\nmessage B { string x = 1; }\n",
    )
    .expect("write dep");

    let snapshot = preload_snapshot(&main, &[]).expect("preload snapshot");

    // Replace and remove the originals after preloading. Any filesystem
    // reopen during compilation would see these and change the result.
    std::fs::write(
        &dep,
        "syntax = \"proto3\";\npackage snap;\nmessage Replaced { string y = 1; }\n",
    )
    .expect("overwrite dep");
    std::fs::remove_file(&main).expect("remove main");

    let pool =
        compile_from_snapshot(&main, snapshot).expect("the frozen snapshot must still compile");
    assert!(
        pool.get_message_by_name("snap.A").is_some(),
        "the frozen root message must resolve"
    );
    assert!(
        pool.get_message_by_name("snap.B").is_some(),
        "the frozen dependency must resolve"
    );
    assert!(
        pool.get_message_by_name("snap.Replaced").is_none(),
        "the replacement contents must not be used"
    );
}
