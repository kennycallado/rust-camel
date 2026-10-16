#![allow(dead_code)]

/// Support utilities for integration tests that need the xml-bridge binary.
///
/// [`resolve_xml_bridge_binary`] fails closed: it honors only an explicit
/// `CAMEL_XML_BRIDGE_BINARY_PATH` and returns `None` otherwise. It never falls
/// back to the `XML_BRIDGE_PATH` alias, a workspace auto-detection, or a
/// downloaded/stale artifact, so a test can only ever exercise the binary the
/// caller explicitly selected.
///
/// Tests call [`require_xml_bridge_binary`], which panics with a clear message
/// naming the env var if it is absent, making the misconfiguration obvious in
/// CI logs instead of a cryptic download/connection error.
use std::path::PathBuf;

pub const ENV_CAMEL: &str = "CAMEL_XML_BRIDGE_BINARY_PATH";
pub const ENV_ALIAS: &str = "XML_BRIDGE_PATH";

/// Returns the path to the xml-bridge binary, or `None` if the explicit
/// override is absent or does not name an existing file.
///
/// Fail closed: when `CAMEL_XML_BRIDGE_BINARY_PATH` is absent this returns
/// `None` — it deliberately does NOT auto-detect a workspace build, honor the
/// `XML_BRIDGE_PATH` alias, or otherwise return a path the process was not
/// explicitly pointed at. That way [`require_xml_bridge_binary`] panics naming
/// the env var rather than a test silently running against a stale or foreign
/// binary.
pub fn resolve_xml_bridge_binary() -> Option<PathBuf> {
    let p = std::env::var(ENV_CAMEL).ok()?;
    let path = PathBuf::from(p);
    path.is_file().then_some(path)
}

/// Ensures the explicit xml-bridge binary override is present and returns it.
///
/// Panics with a clear message naming `CAMEL_XML_BRIDGE_BINARY_PATH` when the
/// override is absent or does not name a file, so the test fails fast with
/// actionable instructions instead of a cryptic download/connection error.
pub fn require_xml_bridge_binary() -> PathBuf {
    resolve_xml_bridge_binary().unwrap_or_else(|| {
        panic!(
            "xml-bridge binary not found: {} is not set to an existing file.\n\
             Build it with:\n  cd bridges/xml && ./build-native.sh\n\
             Then export {} to the built binary path.\n\
             Example: {} bridges/xml/build/native/xml-bridge cargo test ...",
            ENV_CAMEL, ENV_CAMEL, ENV_CAMEL,
        )
    })
}

/// Path of the worktree-native xml-bridge built by `cargo xtask build-xml-bridge`.
///
/// Derived from this crate's manifest dir, so it always resolves inside the
/// checkout that compiled the test binary — never a shared/stale main artifact.
pub fn worktree_native_binary() -> Option<PathBuf> {
    find_workspace_root().map(|root| root.join("bridges/xml/build/native/xml-bridge"))
}

/// Enforces the worktree-native binary for schema-set integration tests.
///
/// Unlike [`require_xml_bridge_binary`], this does NOT mutate the process
/// environment: the test runner exports `CAMEL_XML_BRIDGE_BINARY_PATH` and
/// unsets `XML_BRIDGE_PATH` before launch. If an explicit override is present it
/// must resolve to the same file as the worktree build, so an ambient stale
/// main-checkout path fails closed instead of silently shadowing the rebuild.
/// An absent override also fails closed: without it the component would
/// resolve/download a binary itself, so the test would no longer exercise the
/// worktree native build.
pub fn require_worktree_native_bridge() -> PathBuf {
    let expected = worktree_native_binary().unwrap_or_else(|| {
        panic!(
            "cannot locate the workspace root from {}; expected \
             bridges/xml/build/native/xml-bridge",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    if !expected.is_file() {
        panic!(
            "xml-bridge binary not found at {}.\n\
             Build it with:\n  cargo xtask build-xml-bridge\n\
             (then re-run with {} set and {} unset)",
            expected.display(),
            ENV_CAMEL,
            ENV_ALIAS,
        );
    }
    match std::env::var(ENV_CAMEL) {
        Ok(override_path) => {
            let override_path = PathBuf::from(override_path);
            assert!(
                override_path.is_file(),
                "{} points to a non-existent file: {}; expected the worktree native build at {}",
                ENV_CAMEL,
                override_path.display(),
                expected.display()
            );
            assert_eq!(
                std::fs::canonicalize(&override_path).ok(),
                std::fs::canonicalize(&expected).ok(),
                "{ENV_CAMEL} must point at the worktree native build ({}), not a stale checkout artifact",
                expected.display(),
            );
        }
        // Fail closed: without the explicit override the component under test
        // would resolve/download a binary itself, so these tests would no
        // longer prove anything about the worktree native build.
        Err(_) => panic!(
            "{} is not set; these schema-set tests must run against the worktree \
             native build at {} and never a downloaded or stale artifact.\n\
             Re-run with {} set to that path and {} unset.",
            ENV_CAMEL,
            expected.display(),
            ENV_CAMEL,
            ENV_ALIAS,
        ),
    }
    expected
}

fn find_workspace_root() -> Option<PathBuf> {
    let start = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut current = start;
    for _ in 0..10 {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists()
            && std::fs::read_to_string(&cargo_toml)
                .map(|c| c.contains("[workspace]"))
                .unwrap_or(false)
            && current.join("bridges").exists()
        {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
}
