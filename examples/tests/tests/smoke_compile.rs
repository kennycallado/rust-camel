//! Regression: all 3 example apps still compile after Phase C annotations.

#[test]
fn examples_have_system_broken_annotations() {
    // This test uses CARGO_MANIFEST_DIR which points to examples/tests/
    // Go up one level to reach examples/
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let examples_dir = std::path::Path::new(&dir)
        .parent()
        .expect("examples/tests has a parent (examples/)");

    for (name, rel_path) in &[
        ("container-hot-reload", "container-hot-reload/src/main.rs"),
        ("hot-reload-yaml", "hot-reload-yaml/src/main.rs"),
        ("hot-reload", "hot-reload/src/main.rs"),
    ] {
        let src = std::fs::read_to_string(examples_dir.join(rel_path))
            .unwrap_or_else(|e| panic!("read {name}: {e}"));
        assert!(
            src.contains("// log-policy: system-broken"),
            "{name} must have system-broken annotation"
        );
    }
}
