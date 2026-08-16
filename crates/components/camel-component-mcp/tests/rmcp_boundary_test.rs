use std::fs;
use std::path::{Path, PathBuf};

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).expect("failed to read directory");
    for entry in entries {
        let entry = entry.expect("failed to read directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Mirrors the spec pattern `pub\s+(struct|enum|trait|type|fn|const)\s+Mcp`
/// without pulling in a regex dependency: leading whitespace is ignored, the
/// keyword must be followed by whitespace, and the declaration name must start
/// with `Mcp`.
fn line_declares_public_mcp_type(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("pub") else {
        return false;
    };
    let rest = rest.trim_start();
    for kw in ["struct", "enum", "trait", "type", "fn", "const"] {
        if let Some(after) = rest.strip_prefix(kw) {
            let followed_by_whitespace = after.chars().next().is_some_and(|c| c.is_whitespace());
            return followed_by_whitespace && after.trim_start().starts_with("Mcp");
        }
    }
    false
}

#[test]
fn no_rmcp_outside_adapter() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let adapter_dir = src.join("adapter");
    let mut files = Vec::new();
    collect_rust_files(&src, &mut files);
    let mut violations = Vec::new();
    for file in &files {
        if file.starts_with(&adapter_dir) {
            continue;
        }
        let content = fs::read_to_string(file).expect("failed to read source file");
        if content.contains("rmcp::") {
            violations.push(file.display().to_string());
        }
    }
    assert!(
        violations.is_empty(),
        "rmcp:: must only appear under src/adapter (rmcp boundary): found in: {}",
        violations.join(", ")
    );
}

#[test]
fn camel_api_has_no_mcp_public_types() {
    let api_src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../camel-api/src");
    let mut files = Vec::new();
    collect_rust_files(&api_src, &mut files);
    let mut violations = Vec::new();
    for file in &files {
        let content = fs::read_to_string(file).expect("failed to read camel-api source file");
        for (i, line) in content.lines().enumerate() {
            if line_declares_public_mcp_type(line) {
                violations.push(format!("{}:{}", file.display(), i + 1));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "camel-api must not gain public MCP types: found in: {}",
        violations.join(", ")
    );
}
