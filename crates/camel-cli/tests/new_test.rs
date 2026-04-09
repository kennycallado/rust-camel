use std::fs;

use camel_cli::commands::new::NewArgs;
use camel_cli::template::ProfileLayout;

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
    assert!(hello.contains("timer:tick"));
    assert!(hello.contains("log:"));
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
        stderr.contains("'..'"),
        "expected error about path traversal, got: {stderr}"
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
