//! Integration tests using real binaries. Each test builds config via ExecBundle::from_toml.
use camel_component_api::ComponentBundle;
use camel_component_exec::ExecBundle;

fn cfg(profiles_toml: &str) -> toml::Value {
    let s = format!("workspace_root = \".\"\n{profiles_toml}");
    toml::from_str(&s).unwrap()
}

#[test]
fn bundle_pins_configured_profile() {
    let b = ExecBundle::from_toml(cfg(r#"
[[profiles]]
name = "echo"
executable = "echo"
args = { allow = "any" }
accepted_exit_codes = [0]
timeout_secs = 5
"#))
    .unwrap();
    // Pin happened during validate inside from_toml — Ok means executable was resolved.
    drop(b);
}

#[test]
fn bundle_rejects_empty_profiles_fail_closed() {
    let err = ExecBundle::from_toml(cfg("")).unwrap_err();
    assert!(err.to_string().contains("no profiles"), "{err}");
}

#[test]
fn bundle_rejects_unresolvable_executable() {
    let err = ExecBundle::from_toml(cfg(r#"
[[profiles]]
name = "x"
executable = "definitely-not-a-real-binary-xyz"
args = { allow = "any" }
"#))
    .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("not found")
            || err.to_string().contains("executable"),
        "{err}"
    );
}
