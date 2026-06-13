use std::path::PathBuf;

/// Enforces that no file outside provider/siumai_adapter.rs and
/// provider_factory.rs imports siumai.
#[test]
fn no_siumai_imports_outside_adapter() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowed_files = ["provider/siumai_adapter.rs", "provider_factory.rs"];

    let mut violations = Vec::new();

    let mut stack = vec![crate_root.clone()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).expect("read dir");
        for entry in entries {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }

            let rel = path
                .strip_prefix(&crate_root)
                .expect("strip prefix")
                .to_string_lossy()
                .replace('\\', "/");

            if allowed_files.contains(&rel.as_str()) {
                continue;
            }

            let content = std::fs::read_to_string(&path).expect("read file");
            for (lineno, line) in content.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("//") {
                    continue;
                }
                if trimmed.contains("use siumai")
                    || trimmed.contains("siumai::")
                    || trimmed.contains("siumai_core::")
                    || trimmed.contains("siumai_provider_")
                    || trimmed.contains("siumai_registry::")
                    || trimmed.contains("siumai_bridge::")
                {
                    violations.push(format!("{}:{}: {}", rel, lineno + 1, trimmed));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "siumai imports found outside adapter boundary:\n{}",
        violations.join("\n")
    );
}
