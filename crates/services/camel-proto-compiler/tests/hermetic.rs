//! Hermetic compilation tests.
//!
//! The parent tests re-exec the test binary as a child with a cleared
//! environment (`PATH=""`, no `PROTOC`, a non-existent `TMPDIR`) and assert that
//! every fixture still compiles. A separate scan test proves that the runtime
//! sources expose no executable-spawning surface.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use camel_proto_compiler::compile_proto;
use prost_reflect::DescriptorPool;

/// Compile-time crate root. The child process runs with a cleared environment,
/// so never read `CARGO_MANIFEST_DIR` at runtime.
fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Held by every parent test so a freshly written script is never executed
/// while a sibling test forks (`ETXTBSY`). The race is a Unix artifact, so the
/// lock is Unix-only.
#[cfg(unix)]
static SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquires [`SPAWN_LOCK`] on Unix. Never called on other platforms.
#[cfg(unix)]
fn spawn_guard() -> std::sync::MutexGuard<'static, ()> {
    SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

fn assert_message(pool: &DescriptorPool, name: &str) {
    assert!(
        pool.get_message_by_name(name).is_some(),
        "missing message {name}"
    );
}

fn assert_service(pool: &DescriptorPool, name: &str) {
    assert!(
        pool.get_service_by_name(name).is_some(),
        "missing service {name}"
    );
}

/// Re-executes the current test binary as a hermetic child.
fn run_child(envs: &[(&str, &str)], cwd: Option<&Path>) -> Output {
    // Test-owned parent; the child TMPDIR below it is never created.
    let parent = tempfile::tempdir().expect("tmp parent");
    let missing_tmp = parent.path().join("does-not-exist");
    assert!(
        !missing_tmp.exists(),
        "hermetic child TMPDIR must not exist"
    );
    let mut command = Command::new(std::env::current_exe().expect("current exe"));
    command
        .args([
            "--exact",
            "hermetic_child_body",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("CAMEL_PROTO_HERMETIC_CHILD", "1")
        .env("TMPDIR", &missing_tmp)
        .env("PATH", "");
    for (key, value) in envs {
        command.env(key, value);
    }
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().expect("run hermetic child");
    drop(parent);
    output
}

fn assert_child_ok(output: &Output) {
    assert!(
        output.status.success(),
        "child failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The child body. Outside a hermetic child this is a passing no-op.
#[test]
fn hermetic_child_body() {
    if std::env::var("CAMEL_PROTO_HERMETIC_CHILD").ok().as_deref() != Some("1") {
        return;
    }

    if std::env::var_os("CAMEL_PROTO_HERMETIC_EXPECT_PROTOC").is_none() {
        assert!(
            std::env::var_os("PROTOC").is_none(),
            "PROTOC must be absent in the hermetic child"
        );
    }

    if let Ok(relative) = std::env::var("CAMEL_PROTO_HERMETIC_RELATIVE") {
        let pool = compile_proto(relative.as_str(), std::iter::empty::<&Path>())
            .unwrap_or_else(|e| panic!("compile {relative}: {e}"));
        assert_message(&pool, "helloworld.HelloRequest");
        return;
    }

    let root = manifest();

    let pool = compile_proto(
        root.join("tests/helloworld.proto"),
        std::iter::empty::<&Path>(),
    )
    .expect("compile local helloworld");
    assert_message(&pool, "helloworld.HelloRequest");
    assert_message(&pool, "helloworld.HelloReply");
    assert_service(&pool, "helloworld.Greeter");

    let grpc = root.join("../../components/camel-component-grpc/tests");
    let pool = compile_proto(grpc.join("helloworld.proto"), std::iter::empty::<&Path>())
        .expect("compile gRPC helloworld");
    assert_message(&pool, "helloworld.HelloRequest");
    assert_message(&pool, "helloworld.HelloReply");
    assert_service(&pool, "helloworld.Greeter");

    let pool = compile_proto(grpc.join("streaming.proto"), std::iter::empty::<&Path>())
        .expect("compile gRPC streaming");
    assert_service(&pool, "streaming.StreamService");
    assert_message(&pool, "streaming.ListRequest");
    assert_message(&pool, "streaming.EchoResponse");

    let recursive = root.join("../../dataformats/camel-dataformat-protobuf/tests/fixtures");
    let pool = compile_proto(
        recursive.join("recursive.proto"),
        std::iter::empty::<&Path>(),
    )
    .expect("compile recursive");
    assert_message(&pool, "test.Node");

    let pool = compile_proto(
        root.join("tests/fixtures/ks/main/kitchen.proto"),
        [root.join("tests/fixtures/ks/lib")],
    )
    .expect("compile kitchen sink");
    assert_message(&pool, "kitchen.v1.Order");
    assert_message(&pool, "kitchen.v1.Order.Line");
    assert_message(&pool, "common.Address");
    assert_message(&pool, "legacy.Legacy");
    assert_service(&pool, "kitchen.v1.OrderService");

    // The child inherits the crate root as cwd.
    let pool = compile_proto("tests/helloworld.proto", std::iter::empty::<&Path>())
        .expect("compile relative helloworld");
    assert_service(&pool, "helloworld.Greeter");
}

#[test]
fn compiles_with_no_protoc_no_path_no_tmpdir() {
    #[cfg(unix)]
    let _g = spawn_guard();
    let output = run_child(&[], None);
    assert_child_ok(&output);
}

#[test]
fn empty_parent_relative_path_compiles() {
    #[cfg(unix)]
    let _g = spawn_guard();
    let output = run_child(
        &[("CAMEL_PROTO_HERMETIC_RELATIVE", "helloworld.proto")],
        Some(&manifest().join("tests")),
    );
    assert_child_ok(&output);
}

/// Writes an executable `protoc`-named shell script that appends to a sibling
/// `marker` file when run. Unix-only.
#[cfg(unix)]
fn write_marker_script(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script = dir.join("protoc");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf invoked >> \"${0%/*}/marker\"\nexit 1\n",
    )
    .expect("write protoc script");
    let mut permissions = std::fs::metadata(&script)
        .expect("stat protoc script")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("chmod protoc script");
    script
}

#[cfg(unix)]
#[test]
fn env_override_cannot_bring_back_external_compiler() {
    let _g = spawn_guard();
    let dir = tempfile::tempdir().expect("tmp dir");
    let script = write_marker_script(dir.path());

    // Control: the marker mechanism works.
    let status = Command::new(&script).status().expect("run control script");
    assert!(!status.success(), "control script exits 1 by design");
    let marker = dir.path().join("marker");
    assert!(marker.exists(), "control run must create the marker");
    std::fs::remove_file(&marker).expect("remove marker");

    let dir_str = dir.path().to_str().expect("dir path is UTF-8");
    let script_str = script.to_str().expect("script path is UTF-8");
    let output = run_child(
        &[
            ("PATH", dir_str),
            ("PROTOC", script_str),
            ("CAMEL_PROTO_HERMETIC_EXPECT_PROTOC", "1"),
        ],
        None,
    );
    assert_child_ok(&output);
    assert!(
        !marker.exists(),
        "an external compiler must never be spawned"
    );
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_external_spawn_surface_in_runtime_sources() {
    const TOKENS: [&str; 6] = [
        "Command",
        "tempfile",
        "temp_dir",
        "PROTOC",
        "protoc-bin-vendored",
        "std::process",
    ];

    let mut files = Vec::new();
    collect_rs_files(&manifest().join("src"), &mut files);
    assert!(!files.is_empty(), "src must contain Rust sources");

    for file in &files {
        let text = std::fs::read_to_string(file).expect("read runtime source");
        let pre_test = text
            .lines()
            .take_while(|line| *line != "#[cfg(test)]")
            .collect::<Vec<_>>()
            .join("\n");
        for token in TOKENS {
            assert!(
                !pre_test.contains(token),
                "{} contains forbidden token {token:?} before #[cfg(test)]",
                file.display()
            );
        }
    }

    let cargo = std::fs::read_to_string(manifest().join("Cargo.toml")).expect("read Cargo.toml");
    let runtime_deps = cargo
        .lines()
        .skip_while(|line| *line != "[dependencies]")
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !runtime_deps.contains("protoc-bin-vendored"),
        "runtime dependencies must not include protoc-bin-vendored"
    );
    assert!(
        !runtime_deps.contains("tempfile"),
        "runtime dependencies must not include tempfile"
    );
}
