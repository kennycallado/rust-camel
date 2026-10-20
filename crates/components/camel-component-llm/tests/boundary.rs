use std::path::PathBuf;

/// Enforces that no file outside the allowed boundary imports genai.
///
/// ADR-0020 (amended for the genai swap): the production boundary is
/// `provider/genai_adapter.rs`; the only other exception is its unit-test
/// module `provider/genai_adapter_tests.rs`, which exercises genai types
/// directly. `provider_factory.rs` is deliberately outside the allowlist.
#[test]
fn no_genai_imports_outside_adapter() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowed_files = [
        "provider/genai_adapter.rs",
        "provider/genai_adapter_tests.rs",
    ];

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
                if imports_genai(trimmed) {
                    violations.push(format!("{}:{}: {}", rel, lineno + 1, trimmed));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "genai imports found outside adapter boundary:\n{}",
        violations.join("\n")
    );
}

/// Match `genai::` or `use genai` with a word boundary, so identifiers and
/// module paths such as `genai_adapter` do not match.
fn imports_genai(line: &str) -> bool {
    if line.contains("genai::") {
        return true;
    }

    let needle = "use genai";
    let mut rest = line;
    while let Some(pos) = rest.find(needle) {
        let after = &rest[pos + needle.len()..];
        if !matches!(
            after.chars().next(),
            Some(c) if c.is_alphanumeric() || c == '_'
        ) {
            return true;
        }
        rest = after;
    }

    false
}
