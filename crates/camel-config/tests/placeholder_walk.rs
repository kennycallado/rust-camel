//! Integration tests for `resolve_tree_placeholders` — the recursive TOML
//! walk with strict/plain leaf dispatch. These cover the public walk API in
//! isolation; the rewired load path is covered by `placeholder_e2e.rs`.

use camel_config::config::resolve_tree_placeholders;
use toml::Value;

mod common;

/// Sets a uniquely-named env var and removes it on drop (panic-safe restore).
struct EnvCleanup(&'static str);

impl EnvCleanup {
    fn set(name: &'static str, value: &str) -> Self {
        unsafe { std::env::set_var(name, value) };
        EnvCleanup(name)
    }
}

impl Drop for EnvCleanup {
    fn drop(&mut self) {
        unsafe { std::env::remove_var(self.0) };
    }
}

fn tree(raw: &str) -> Value {
    toml::from_str(raw).expect("test fixture must be valid TOML")
}

fn leaf<'a>(root: &'a Value, keys: &[&str]) -> &'a str {
    let (last, path) = keys.split_last().expect("keys must be non-empty");
    let mut node = root;
    for k in path {
        node = node
            .get(k)
            .unwrap_or_else(|| panic!("fixture must contain key `{k}`"));
        assert!(node.is_table(), "key `{k}` must be a table, got {node:?}");
    }
    node.get(last)
        .unwrap_or_else(|| panic!("fixture must contain key `{last}`"))
        .as_str()
        .unwrap_or_else(|| panic!("fixture leaf `{last}` must be a string"))
}

#[test]
fn strict_leaf_resolves_and_fails_closed() {
    let _guard = common::env_lock();
    let _env = EnvCleanup::set("RUST_CAMEL_TEST_WALK_A", "tok-123");
    let mut cfg = tree(
        r#"[security.native]
        bearer_token = "${env:RUST_CAMEL_TEST_WALK_A}"
        "#,
    );
    resolve_tree_placeholders(&mut cfg).expect("set var must resolve");
    assert_eq!(
        leaf(&cfg, &["security", "native", "bearer_token"]),
        "tok-123"
    );
    drop(_env);

    let mut cfg = tree(
        r#"[security.native]
        bearer_token = "${env:RUST_CAMEL_TEST_WALK_A}"
        "#,
    );
    let err = resolve_tree_placeholders(&mut cfg).expect_err("unset var must fail closed");
    assert!(
        err.to_string().contains("security.native.bearer_token"),
        "error must name the field, got: {err}"
    );
}

#[test]
fn plain_leaf_uniform_fail_closed() {
    let _guard = common::env_lock();
    unsafe { std::env::remove_var("RUST_CAMEL_TEST_WALK_B") };
    let mut cfg = tree(
        r#"[observability.otel]
        endpoint = "${env:RUST_CAMEL_TEST_WALK_B}"
        "#,
    );
    let err = resolve_tree_placeholders(&mut cfg).expect_err("unset var must fail closed");
    assert!(
        err.to_string().contains("observability.otel.endpoint"),
        "error must name the field, got: {err}"
    );
}

#[test]
fn plain_leaf_passthrough_without_markers() {
    let mut cfg = tree(
        r#"log_level = "hello"

        [components.timer]
        period = "${body}"
        "#,
    );
    resolve_tree_placeholders(&mut cfg).expect("leaves without markers pass through");
    assert_eq!(leaf(&cfg, &["components", "timer", "period"]), "${body}");
    assert_eq!(leaf(&cfg, &["log_level"]), "hello");
}

#[test]
fn strict_leaf_matrix_covers_spec_scenarios() {
    let _guard = common::env_lock();
    unsafe { std::env::remove_var("RUST_CAMEL_TEST_MTX_A") };
    unsafe { std::env::remove_var("RUST_CAMEL_TEST_MTX_C") };
    let _b = EnvCleanup::set("RUST_CAMEL_TEST_MTX_B", "oidc-secret-1");
    let _d = EnvCleanup::set("RUST_CAMEL_TEST_MTX_D", "postgres://db/main");
    let _e = EnvCleanup::set("RUST_CAMEL_TEST_MTX_E", "extra-pass-1");
    let _f = EnvCleanup::set("RUST_CAMEL_TEST_MTX_F", "redis://idem:6379");
    let _g = EnvCleanup::set("RUST_CAMEL_TEST_MTX_G", "sentinel-pass-1");

    let mut cfg = tree(
        r#"
        [security.native]
        bearer_token = "${env:RUST_CAMEL_TEST_MTX_A:-fallback-tok}"

        [security.oidc]
        client_secret = "${env:RUST_CAMEL_TEST_MTX_B}"

        [security.keycloak]
        realm = "${env:RUST_CAMEL_TEST_MTX_C:-main}"

        [datasources.main]
        db_url = "${env:RUST_CAMEL_TEST_MTX_D}"

        [datasources.main.extra]
        password = "${env:RUST_CAMEL_TEST_MTX_E}"

        [idempotent_repo]
        url = "${env:RUST_CAMEL_TEST_MTX_F}"

        [cache_repo]
        sentinel_password = "${env:RUST_CAMEL_TEST_MTX_G}"
        "#,
    );
    resolve_tree_placeholders(&mut cfg).expect("all vars set or defaulted must resolve");
    assert_eq!(
        leaf(&cfg, &["security", "native", "bearer_token"]),
        "fallback-tok"
    );
    assert_eq!(
        leaf(&cfg, &["security", "oidc", "client_secret"]),
        "oidc-secret-1"
    );
    assert_eq!(leaf(&cfg, &["security", "keycloak", "realm"]), "main");
    assert_eq!(
        leaf(&cfg, &["datasources", "main", "db_url"]),
        "postgres://db/main"
    );
    assert_eq!(
        leaf(&cfg, &["datasources", "main", "extra", "password"]),
        "extra-pass-1"
    );
    assert_eq!(leaf(&cfg, &["idempotent_repo", "url"]), "redis://idem:6379");
    assert_eq!(
        leaf(&cfg, &["cache_repo", "sentinel_password"]),
        "sentinel-pass-1"
    );
}

#[test]
fn keycloak_secret_fails_closed_when_missing() {
    let _guard = common::env_lock();
    unsafe { std::env::remove_var("RUST_CAMEL_TEST_KC_MISS") };
    let mut cfg = tree(
        r#"[security.keycloak]
        client_secret = "${env:RUST_CAMEL_TEST_KC_MISS}"
        "#,
    );
    let err = resolve_tree_placeholders(&mut cfg).expect_err("unset var must fail closed");
    assert!(
        err.to_string().contains("security.keycloak.client_secret"),
        "error must name the field, got: {err}"
    );
}

#[test]
// Unguarded by design: rejection fires at form-validation, before any
// env lookup. If validation is ever reordered after env resolution,
// add common::env_lock() (see rc-tdae / e_glm review).
fn legacy_braces_rejected_everywhere() {
    let mut plain = tree(r#"log_level = "{{env:X}}""#);
    let err = resolve_tree_placeholders(&mut plain)
        .expect_err("legacy braces must be rejected on plain leaves");
    assert!(
        err.to_string().contains("${env:NAME}"),
        "message must point at the ${{env:}} replacement forms, got: {err}"
    );

    let mut strict = tree(
        r#"[security.native]
        bearer_token = "{{env:X}}"
        "#,
    );
    let err = resolve_tree_placeholders(&mut strict)
        .expect_err("legacy braces must be rejected on strict leaves");
    assert!(
        err.to_string().contains("${env:NAME}"),
        "message must point at the ${{env:}} replacement forms, got: {err}"
    );
}

#[test]
fn standalone_dollar_converts_on_all_leaf_classes() {
    let mut cfg = tree(
        r#"
        log_level = "a$$b"

        [security.keycloak]
        realm = "a$$b"

        [datasources.main]
        provider = "a$$b"

        [idempotent_repo]
        backend = "a$$b"

        [cache_repo]
        backend = "a$$b"
        "#,
    );
    resolve_tree_placeholders(&mut cfg).expect("standalone $$ must convert everywhere");
    assert_eq!(leaf(&cfg, &["log_level"]), "a$b");
    assert_eq!(leaf(&cfg, &["security", "keycloak", "realm"]), "a$b");
    assert_eq!(leaf(&cfg, &["datasources", "main", "provider"]), "a$b");
    assert_eq!(leaf(&cfg, &["idempotent_repo", "backend"]), "a$b");
    assert_eq!(leaf(&cfg, &["cache_repo", "backend"]), "a$b");
}

#[test]
fn escaped_full_form_rejected_on_strict_leaves() {
    let _guard = common::env_lock();
    unsafe { std::env::remove_var("RUST_CAMEL_TEST_WALK_C") };
    unsafe { std::env::remove_var("RUST_CAMEL_TEST_WALK_E") };
    let mut security = tree(
        r#"[security.native]
        bearer_token = "$${env:RUST_CAMEL_TEST_WALK_C}"
        "#,
    );
    let err = resolve_tree_placeholders(&mut security)
        .expect_err("escaped form must die on strict leaves");
    assert!(
        err.to_string().contains("unresolved placeholder marker"),
        "residual gate must kill the literal, got: {err}"
    );

    let mut repo = tree(
        r#"[idempotent_repo]
        sentinel_password = "$${env:RUST_CAMEL_TEST_WALK_E}"
        "#,
    );
    let err =
        resolve_tree_placeholders(&mut repo).expect_err("escaped form must die on strict leaves");
    assert!(
        err.to_string().contains("unresolved placeholder marker"),
        "residual gate must kill the literal, got: {err}"
    );
}

#[test]
fn escaped_full_form_literal_on_plain_leaves() {
    let _guard = common::env_lock();
    unsafe { std::env::remove_var("RUST_CAMEL_TEST_WALK_D") };
    let mut cfg = tree(r#"log_level = "$${env:RUST_CAMEL_TEST_WALK_D}""#);
    resolve_tree_placeholders(&mut cfg).expect("escaped form is legal on plain leaves");
    assert_eq!(leaf(&cfg, &["log_level"]), "${env:RUST_CAMEL_TEST_WALK_D}");
}

#[test]
fn repository_leaves_follow_strict_gate() {
    let _guard = common::env_lock();
    // (a) Both repository leaves resolve when their vars are set.
    let _u = EnvCleanup::set("RUST_CAMEL_TEST_REPO_U", "redis://idem:6379");
    let _p = EnvCleanup::set("RUST_CAMEL_TEST_REPO_P", "sentinel-pass-1");
    let mut cfg = tree(
        r#"
        [idempotent_repo]
        url = "${env:RUST_CAMEL_TEST_REPO_U}"

        [cache_repo]
        sentinel_password = "${env:RUST_CAMEL_TEST_REPO_P}"
        "#,
    );
    resolve_tree_placeholders(&mut cfg).expect("set vars must resolve");
    assert_eq!(leaf(&cfg, &["idempotent_repo", "url"]), "redis://idem:6379");
    assert_eq!(
        leaf(&cfg, &["cache_repo", "sentinel_password"]),
        "sentinel-pass-1"
    );
    drop(_u);
    drop(_p);

    // (b) Unset var on a repository leaf fails closed naming the field.
    unsafe { std::env::remove_var("RUST_CAMEL_TEST_REPO_P") };
    let mut cfg = tree(
        r#"[cache_repo]
        sentinel_password = "${env:RUST_CAMEL_TEST_REPO_P}"
        "#,
    );
    let err = resolve_tree_placeholders(&mut cfg).expect_err("unset var must fail closed");
    assert!(
        err.to_string().contains("cache_repo.sentinel_password"),
        "error must name the field, got: {err}"
    );

    // (c) Escaped full forms die on repository leaves via the residual gate.
    // Each leaf gets its own resolve: the walk returns on the first error, so
    // a combined tree would never exercise the second leaf.
    let mut cfg = tree(
        r#"[idempotent_repo]
        url = "$${env:RUST_CAMEL_TEST_REPO_U}"
        "#,
    );
    let err = resolve_tree_placeholders(&mut cfg)
        .expect_err("escaped form must die on repository leaves");
    assert!(
        err.to_string().contains("unresolved placeholder marker"),
        "residual gate must kill the literal, got: {err}"
    );
    assert!(
        err.to_string().contains("idempotent_repo.url"),
        "error must name the field, got: {err}"
    );

    let mut cfg = tree(
        r#"[cache_repo]
        sentinel_password = "$${env:RUST_CAMEL_TEST_REPO_P}"
        "#,
    );
    let err = resolve_tree_placeholders(&mut cfg)
        .expect_err("escaped form must die on repository leaves");
    assert!(
        err.to_string().contains("unresolved placeholder marker"),
        "residual gate must kill the literal, got: {err}"
    );
    assert!(
        err.to_string().contains("cache_repo.sentinel_password"),
        "error must name the field, got: {err}"
    );
}

#[test]
fn strict_dispatch_is_exhaustive_over_security_subtree() {
    let _guard = common::env_lock();
    unsafe { std::env::remove_var("RUST_CAMEL_TEST_GUARD_A") };
    let mut cfg = tree(
        r#"[security.brand_new_section.deep]
        token = "${env:RUST_CAMEL_TEST_GUARD_A}"
        "#,
    );
    let err = resolve_tree_placeholders(&mut cfg)
        .expect_err("unknown security subtree must still resolve strictly");
    assert!(
        err.to_string()
            .contains("security.brand_new_section.deep.token"),
        "strictness must reach unknown depth by path prefix, got: {err}"
    );

    // Escaped residue at the same unknown depth: strict kills it, plain would
    // return Ok with the literal — this discriminates routing.
    let mut cfg = tree(
        r#"[security.brand_new_section.deep]
        token = "$${env:RUST_CAMEL_TEST_GUARD_C}"
        "#,
    );
    let err = resolve_tree_placeholders(&mut cfg)
        .expect_err("escaped residue must die on unknown security subtree");
    assert!(
        err.to_string().contains("unresolved placeholder marker"),
        "residual gate must kill the escaped literal, got: {err}"
    );
}

#[test]
// Unguarded by design: rejection fires at form-validation, before any
// env lookup. If validation is ever reordered after env resolution,
// add common::env_lock() (see rc-tdae / e_glm review).
fn strict_residual_rejects_malformed_dollar_forms() {
    for value in ["${env:", "${notenv:x}"] {
        let mut cfg = tree(&format!(
            r#"[security.native]
            bearer_token = "{value}"
            "#
        ));
        let err = resolve_tree_placeholders(&mut cfg)
            .expect_err("malformed dollar form must die on strict leaves");
        assert!(
            err.to_string().contains("unresolved placeholder marker"),
            "residual gate must reject the malformed form, got: {err}"
        );
    }
}

#[test]
fn valid_new_syntax_passes_strict_gate() {
    let _guard = common::env_lock();
    let _env = EnvCleanup::set("RUST_CAMEL_TEST_GUARD_B", "kc-secret-9");
    let mut cfg = tree(
        r#"[security.keycloak]
        client_secret = "${env:RUST_CAMEL_TEST_GUARD_B}"
        "#,
    );
    resolve_tree_placeholders(&mut cfg).expect("set var must resolve");
    assert_eq!(
        leaf(&cfg, &["security", "keycloak", "client_secret"]),
        "kc-secret-9"
    );
}
