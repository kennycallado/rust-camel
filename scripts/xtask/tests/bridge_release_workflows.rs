//! Release workflow and quality-gate registration tests for bridge versions.

use std::process::Command;

use noyalib::compat::serde_yaml::{self, Value};

const RELEASE_WORKFLOWS: [&str; 3] = [
    ".github/workflows/cxf-bridge-release.yml",
    ".github/workflows/jms-bridge-release.yml",
    ".github/workflows/xml-bridge-release.yml",
];

const GUARD_RUN: &str = "cargo xtask lint-bridge-versions --tag \"$GITHUB_REF_NAME\"";

fn repository_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("missing YAML key '{key}'"))
}

fn release_workflow(path: &str) -> Value {
    let root = repository_root();
    let contents = std::fs::read_to_string(root.join(path))
        .unwrap_or_else(|error| panic!("read {path}: {error}"));
    serde_yaml::from_str::<Value>(&contents).unwrap_or_else(|error| panic!("parse {path}: {error}"))
}

#[test]
fn workflows_parse_and_build_needs_guard() {
    for path in RELEASE_WORKFLOWS {
        let workflow = release_workflow(path);
        let guard = get(get(&workflow, "jobs"), "guard");
        let steps = get(guard, "steps")
            .as_sequence()
            .unwrap_or_else(|| panic!("{path}: guard.steps is not a sequence"));

        let has_checkout = steps.iter().any(|step| {
            step.get("uses")
                .and_then(Value::as_str)
                .is_some_and(|uses| uses.starts_with("actions/checkout@"))
        });
        let has_toolchain = steps.iter().any(|step| {
            step.get("uses")
                .and_then(Value::as_str)
                .is_some_and(|uses| uses.starts_with("dtolnay/rust-toolchain@"))
        });
        let has_guard_run = steps
            .iter()
            .any(|step| step.get("run").and_then(Value::as_str) == Some(GUARD_RUN));

        assert!(has_checkout, "{path}: guard has no actions/checkout step");
        assert!(
            has_toolchain,
            "{path}: guard has no dtolnay/rust-toolchain step"
        );
        assert!(has_guard_run, "{path}: guard does not run {GUARD_RUN}");

        let build = get(get(&workflow, "jobs"), "build");
        assert_eq!(
            get(build, "needs").as_str(),
            Some("guard"),
            "{path}: build does not need guard"
        );
    }
}

#[test]
fn gate_registration_present() {
    let root = repository_root();
    for path in [".github/workflows/ci.yml", "AGENTS.md"] {
        let contents = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|error| panic!("read {path}: {error}"));
        assert!(
            contents.contains("cargo xtask lint-bridge-versions"),
            "{path}: missing cargo xtask lint-bridge-versions"
        );
    }
}

fn write_file(root: &std::path::Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create fixture directory");
    }
    std::fs::write(path, contents).expect("write fixture file");
}

fn constant(version: &str) -> String {
    format!("pub const BRIDGE_VERSION: &str = \"{version}\";\n")
}

fn cli_fixture() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("tempdir");
    write_file(dir.path(), "Cargo.toml", "[workspace]\nmembers = []\n");
    write_file(
        dir.path(),
        "bridges/versions.toml",
        "cxf = \"0.8.4\"\njms = \"0.8.4\"\nxml = \"0.8.1\"\n",
    );
    write_file(
        dir.path(),
        "crates/components/camel-cxf/src/lib.rs",
        &constant("0.8.4"),
    );
    write_file(
        dir.path(),
        "crates/components/camel-jms/src/lib.rs",
        &constant("0.8.4"),
    );
    write_file(
        dir.path(),
        "crates/components/camel-xslt/src/lib.rs",
        &constant("0.8.1"),
    );
    write_file(
        dir.path(),
        "crates/components/camel-validator/src/lib.rs",
        &constant("0.8.1"),
    );
    dir
}

#[test]
fn bump_cli_success_and_failure_use_fixture() {
    let dir = cli_fixture();
    let registry = dir.path().join("bridges/versions.toml");

    let success = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["bump-bridge", "xml", "0.8.2"])
        .current_dir(dir.path())
        .output()
        .expect("run bump-bridge success fixture");
    assert!(
        success.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&success.stderr)
    );
    let stdout = String::from_utf8_lossy(&success.stdout);
    assert!(stdout.contains("bumped xml to 0.8.2:"), "{stdout}");
    assert!(stdout.contains("bridges/versions.toml"), "{stdout}");
    assert!(stdout.contains("camel-xslt/src/lib.rs"), "{stdout}");
    assert!(stdout.contains("camel-validator/src/lib.rs"), "{stdout}");
    assert_eq!(
        std::fs::read_to_string(&registry).expect("read fixture registry"),
        "cxf = \"0.8.4\"\njms = \"0.8.4\"\nxml = \"0.8.2\"\n",
        "the fixture registry must change, never the repository"
    );

    let failure = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["bump-bridge", "http", "0.8.2"])
        .current_dir(dir.path())
        .output()
        .expect("run bump-bridge failure fixture");
    assert!(
        !failure.status.success(),
        "invalid bridge must exit non-zero"
    );
    assert!(
        String::from_utf8_lossy(&failure.stderr).contains("invalid bridge"),
        "stderr: {}",
        String::from_utf8_lossy(&failure.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&registry).expect("read fixture registry after failure"),
        "cxf = \"0.8.4\"\njms = \"0.8.4\"\nxml = \"0.8.2\"\n",
        "failed bump must not write"
    );
}
