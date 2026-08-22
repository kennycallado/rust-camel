mod common;

use std::fs;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use camel_cli::commands::new::NewArgs;
use camel_cli::template::ProfileLayout;

use common::{KillOnDrop, drain_to_buffer, send_term, wait_for_marker};

fn run_new(name: &str, template: &str, force: bool, profile_layout: ProfileLayout) {
    camel_cli::commands::new::run_new(NewArgs {
        name: name.to_string(),
        template: template.to_string(),
        force,
        profile_layout,
    });
}

#[test]
fn creates_project_with_env_layout() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("my-integration");
    let name = project_path.to_str().unwrap();

    run_new(name, "basic", false, ProfileLayout::Env);

    assert!(project_path.join("Camel.toml").exists());
    assert!(project_path.join("routes/hello.yaml").exists());
    assert!(project_path.join("README.md").exists());
    assert!(project_path.join(".gitignore").exists());

    let camel_toml = fs::read_to_string(project_path.join("Camel.toml")).unwrap();
    assert!(camel_toml.contains("[default]"));
    assert!(camel_toml.contains("[development]"));
    assert!(camel_toml.contains("[production]"));
    assert!(camel_toml.contains("routes = [\"routes/*.yaml\"]"));
}

#[test]
fn creates_project_with_simple_layout() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("simple-proj");
    let name = project_path.to_str().unwrap();

    run_new(name, "basic", false, ProfileLayout::Simple);

    let camel_toml = fs::read_to_string(project_path.join("Camel.toml")).unwrap();
    assert!(camel_toml.contains("[default]"));
    assert!(!camel_toml.contains("[development]"));
    assert!(!camel_toml.contains("[production]"));
}

#[test]
fn fails_if_directory_exists_and_not_empty() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("existing-proj");
    fs::create_dir_all(&project_path).unwrap();
    fs::write(project_path.join("existing.txt"), "data").unwrap();

    let name = project_path.to_str().unwrap();

    // run_new calls process::exit(1), so we test via the actual camel binary
    let bin = std::env::var("CAMEL_BIN_PATH").unwrap_or_else(|_| {
        // The test binary lives in target/debug/deps/<hash>; navigate up to find camel
        let mut path = std::env::current_exe().unwrap();
        path.pop(); // remove test binary name (e.g. new_test-<hash>)
        path.pop(); // pop 'deps'
        path.push("camel");
        path.to_str().unwrap().to_string()
    });

    let output = std::process::Command::new(&bin)
        .args(["new", name, "--template", "basic"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "camel new should fail on non-empty dir"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already exists and is not empty"),
        "expected error message about existing directory, got: {stderr}"
    );
}

#[test]
fn force_overwrites_existing_files() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("force-proj");
    let name = project_path.to_str().unwrap();

    run_new(name, "basic", false, ProfileLayout::Env);

    let old_content = fs::read_to_string(project_path.join("Camel.toml")).unwrap();
    fs::write(project_path.join("Camel.toml"), "corrupted").unwrap();

    run_new(name, "basic", true, ProfileLayout::Env);

    let new_content = fs::read_to_string(project_path.join("Camel.toml")).unwrap();
    assert_eq!(new_content, old_content);
    assert_ne!(new_content, "corrupted");
}

#[test]
fn force_preserves_extra_files() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("preserve-proj");
    let name = project_path.to_str().unwrap();

    run_new(name, "basic", false, ProfileLayout::Env);

    fs::write(project_path.join("my-notes.txt"), "important data").unwrap();

    run_new(name, "basic", true, ProfileLayout::Env);

    assert!(project_path.join("my-notes.txt").exists());
    assert_eq!(
        fs::read_to_string(project_path.join("my-notes.txt")).unwrap(),
        "important data"
    );
}

#[test]
fn readme_contains_project_name() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("cool-project");
    let name = project_path.to_str().unwrap();

    run_new(name, "basic", false, ProfileLayout::Env);

    let readme = fs::read_to_string(project_path.join("README.md")).unwrap();
    assert!(readme.contains("# cool-project"));
    assert!(!readme.contains("<name>"));
}

#[test]
fn hello_yaml_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("yaml-proj");
    let name = project_path.to_str().unwrap();

    run_new(name, "basic", false, ProfileLayout::Env);

    let hello = fs::read_to_string(project_path.join("routes/hello.yaml")).unwrap();
    assert!(hello.contains("id: \"hello\""));
    assert!(hello.contains("direct:start"));
    assert!(hello.contains("mock:result"));
    assert!(!hello.contains("timer:tick"));
}

#[test]
fn creates_in_empty_existing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("empty-dir");
    fs::create_dir_all(&project_path).unwrap();

    let name = project_path.to_str().unwrap();
    run_new(name, "basic", false, ProfileLayout::Env);

    assert!(project_path.join("Camel.toml").exists());
}

#[test]
fn unknown_template_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("tmpl-proj");
    let name = project_path.to_str().unwrap();

    let bin = std::env::var("CAMEL_BIN_PATH").unwrap_or_else(|_| {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("camel");
        path.to_str().unwrap().to_string()
    });

    let output = std::process::Command::new(&bin)
        .args(["new", name, "--template", "nonexistent"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "camel new should fail with unknown template"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown template"),
        "expected error about unknown template, got: {stderr}"
    );
}

#[test]
fn rejects_path_traversal_in_name() {
    let bin = std::env::var("CAMEL_BIN_PATH").unwrap_or_else(|_| {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("camel");
        path.to_str().unwrap().to_string()
    });

    let output = std::process::Command::new(&bin)
        .args(["new", "../evil"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "camel new should reject path traversal"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("'..'")
            || stderr.contains("alphanumeric")
            || stderr.contains("invalid value"),
        "expected error about path traversal or invalid name, got: {stderr}"
    );
}

#[test]
fn stdout_shows_next_steps() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("stdout-proj");
    let name = project_path.to_str().unwrap();

    let bin = std::env::var("CAMEL_BIN_PATH").unwrap_or_else(|_| {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("camel");
        path.to_str().unwrap().to_string()
    });

    let output = std::process::Command::new(&bin)
        .args(["new", name])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Created camel project: stdout-proj"),
        "got: {stdout}"
    );
    assert!(stdout.contains("Next steps:"), "got: {stdout}");
    assert!(stdout.contains("camel run"), "got: {stdout}");
    assert!(stdout.contains("camel run --watch"), "got: {stdout}");
}

#[test]
fn creates_project_in_nested_path() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("sub").join("my-proj");
    let name = nested.to_str().unwrap();

    run_new(name, "basic", false, ProfileLayout::Env);

    assert!(nested.join("Camel.toml").exists());
    assert!(nested.join("routes/hello.yaml").exists());
    assert!(nested.join("README.md").exists());

    let readme = fs::read_to_string(nested.join("README.md")).unwrap();
    assert!(readme.contains("# my-proj"));
}

#[test]
fn creates_project_with_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("abs-proj");
    let name = project_path.to_str().unwrap();

    run_new(name, "basic", false, ProfileLayout::Env);

    assert!(project_path.join("Camel.toml").exists());
    let readme = fs::read_to_string(project_path.join("README.md")).unwrap();
    assert!(readme.contains("# abs-proj"));
}

// ---------------------------------------------------------------------------
// Scaffold teaches the camel test placement contract
// (see openspec change test-placement-contract, task 3.1)
// ---------------------------------------------------------------------------

/// The scaffold ships a colocated camel test document next to the route.
#[test]
fn scaffolds_colocated_test_doc() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("test-doc-proj");
    let name = project_path.to_str().unwrap();

    run_new(name, "basic", false, ProfileLayout::Env);

    let test_doc = project_path.join("routes/hello.test.yaml");
    assert!(
        test_doc.exists(),
        "routes/hello.test.yaml must be scaffolded"
    );
    let content = fs::read_to_string(&test_doc).unwrap();
    assert!(
        content.contains("routeFiles: [hello.yaml]"),
        "expected colocated doc to reference the route file, got:\n{content}"
    );
}

/// The generated README teaches `camel test` before `camel run`.
#[test]
fn readme_teaches_test_before_run() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("readme-proj");
    let name = project_path.to_str().unwrap();

    run_new(name, "basic", false, ProfileLayout::Env);

    let readme = fs::read_to_string(project_path.join("README.md")).unwrap();
    let test_idx = readme
        .find("## Test")
        .expect("README must contain a ## Test section");
    let run_idx = readme
        .find("## Run")
        .expect("README must contain a ## Run section");
    assert!(
        test_idx < run_idx,
        "## Test must appear before ## Run, got README:\n{readme}"
    );
}

/// The scaffolded colocated test document actually passes against the
/// scaffolded route, via the same in-process entry `camel test` uses.
#[tokio::test(flavor = "multi_thread")]
async fn scaffolded_test_doc_passes() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("scaffold-test-proj");
    let name = project_path.to_str().unwrap();

    run_new(name, "basic", false, ProfileLayout::Env);

    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = camel_cli::commands::test::run_tests(
        &[project_path.join("routes/hello.test.yaml")],
        &mut out,
        &mut err,
    )
    .await;
    let out = String::from_utf8_lossy(&out);
    let err = String::from_utf8_lossy(&err);
    assert_eq!(summary.exit_code, 0, "stdout:\n{out}\nstderr:\n{err}");
    assert_eq!(summary.failed, 0, "stdout:\n{out}\nstderr:\n{err}");
}

/// Wildcard discovery over the scaffolded `routes/` sees exactly one route
/// (the test document is skipped by the reserved-suffix rule).
#[test]
fn scaffolded_project_run_discovery_skips_test_doc() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("discovery-proj");
    let name = project_path.to_str().unwrap();

    run_new(name, "basic", false, ProfileLayout::Env);

    let pattern = format!("{}/routes/*.yaml", project_path.display());
    let routes = camel_dsl::discover_routes(&[pattern]).expect("discovery must succeed");
    assert_eq!(routes.len(), 1, "only hello.yaml is a route");
    assert_eq!(routes[0].route_id(), "hello");
}

/// The scaffolded project (route + colocated test doc + wildcard
/// `routes = ["routes/*.yaml"]`) runs under `camel run` without the test
/// document tripping discovery. Pattern mirrors `run_exec_guard_test.rs`:
/// piped stdio drained by reader threads, a startup marker wait, then exactly
/// ONE SIGTERM and a bounded wait for termination.
#[test]
fn scaffolded_project_runs_under_camel_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project_path = dir.path().join("run-proj");
    let name = project_path
        .to_str()
        .expect("project path is utf-8")
        .to_string();

    run_new(&name, "basic", false, ProfileLayout::Env);

    let mut child = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_camel"))
            .arg("run")
            .arg("--no-watch")
            .current_dir(&project_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .spawn()
            .expect("failed to spawn `camel` binary"),
    );

    let stdout = child.stdout.take().expect("child stdout was piped");
    let stderr = child.stderr.take().expect("child stderr was piped");
    let stdout_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let stderr_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let out_handle = {
        let buf = Arc::clone(&stdout_buf);
        thread::spawn(move || drain_to_buffer(stdout, buf))
    };
    let err_handle = {
        let buf = Arc::clone(&stderr_buf);
        thread::spawn(move || drain_to_buffer(stderr, buf))
    };

    // Wait (up to 30 s) for the startup marker proving the context and its
    // routes started. The 30 s ceiling rationale lives on
    // `common::wait_for_marker`.
    let observed = wait_for_marker(
        &mut child,
        &[Arc::clone(&stdout_buf)],
        "context started",
        Duration::from_secs(30),
    );
    assert!(
        observed,
        "did not observe `context started` within 30s; captured stdout:\n{}",
        stdout_buf.lock().expect("buffer lock poisoned")
    );

    // Send exactly ONE SIGTERM. A second signal would force exit(1).
    send_term(&child);

    // Bounded wait for termination; force-kill at the deadline (the kill-on-
    // drop guard makes that force-kill best-effort idempotent).
    let step = Duration::from_millis(25);
    let term_deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= term_deadline => {
                panic!("camel run did not terminate within 30s after SIGTERM");
            }
            Ok(None) => thread::sleep(step),
            Err(e) => panic!("try_wait failed: {e}"),
        }
    }

    let _ = out_handle.join();
    let _ = err_handle.join();
    let captured_stderr = stderr_buf.lock().expect("buffer lock poisoned").clone();
    assert!(
        !captured_stderr.contains("hello.test.yaml"),
        "camel run stderr must not name the colocated test doc; stderr:\n{captured_stderr}"
    );
}
