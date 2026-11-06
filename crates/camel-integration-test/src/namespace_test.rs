//! Document-namespace mint tests (bd rc-ql8eq, task 2.1).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! `#[cfg(test)]`. Every test is hermetic: run ids are fixed literals
//! except the per-invocation test, and the ambient lookup is an injected
//! closure, so no test reads the real process environment.

use crate::env_layers::AmbientLookup;
use crate::env_layers::LayeredEnv;
use crate::namespace;
use crate::namespace::NamespaceBinding;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Ambient lookup that resolves nothing, so the harness layer is the only
/// visible source in the binding test.
fn no_ambient() -> AmbientLookup {
    Arc::new(|_| None)
}

#[test]
fn namespace_mint_is_prefix_free_across_documents() {
    let run_id = "3f9a1c0e";
    let mint10 = namespace::mint_namespace("112", run_id, 10).expect("ordinal 10 is in range");
    let mint100 = namespace::mint_namespace("112", run_id, 100).expect("ordinal 100 is in range");

    assert_ne!(mint10, mint100);
    assert!(
        !mint10.starts_with(&mint100),
        "ordinal 10 must not be a prefix of ordinal 100: {mint10} vs {mint100}"
    );
    assert!(
        !mint100.starts_with(&mint10),
        "ordinal 100 must not be a prefix of ordinal 10: {mint100} vs {mint10}"
    );
    assert!(mint10.ends_with('~'), "mint must end with '~': {mint10}");
    assert!(mint100.ends_with('~'), "mint must end with '~': {mint100}");
    assert!(
        namespace::is_valid_namespace_value(&mint10),
        "mint must satisfy the grammar: {mint10}"
    );
    assert!(
        namespace::is_valid_namespace_value(&mint100),
        "mint must satisfy the grammar: {mint100}"
    );
}

#[test]
fn namespace_is_minted_per_document_and_bound() {
    let bindings = [NamespaceBinding {
        bind_var: "ns".to_string(),
        base: "112".to_string(),
    }];
    let run_id = "3f9a1c0e";

    let mut first = BTreeMap::new();
    namespace::apply_namespaces(&mut first, &bindings, run_id, 1).expect("ordinal 1 mints");
    let mut second = BTreeMap::new();
    namespace::apply_namespaces(&mut second, &bindings, run_id, 2).expect("ordinal 2 mints");

    let first_ns = first.get("ns").cloned().expect("first document binds ns");
    let second_ns = second.get("ns").cloned().expect("second document binds ns");
    assert_ne!(first_ns, second_ns, "each document gets its own mint");
    assert!(
        namespace::is_valid_namespace_value(&first_ns),
        "first mint must satisfy the grammar: {first_ns}"
    );
    assert!(
        namespace::is_valid_namespace_value(&second_ns),
        "second mint must satisfy the grammar: {second_ns}"
    );

    // The doc map is deliberately empty: the mint is resolved from the
    // harness layer, never from the document `env:` map.
    let env = LayeredEnv::new(BTreeMap::new(), first, Vec::new(), no_ambient());
    assert_eq!(
        env.lookup("ns").as_deref(),
        Some(first_ns.as_str()),
        "lookup must return this document's mint"
    );
}

#[test]
fn namespace_run_id_is_per_invocation() {
    let first = namespace::generate_run_id();
    let second = namespace::generate_run_id();

    for run_id in [&first, &second] {
        assert_eq!(
            run_id.len(),
            namespace::RUN_ID_HEX_LEN,
            "run id must be exactly eight hex chars: {run_id}"
        );
        assert!(
            run_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "run id must be lowercase hex: {run_id}"
        );
    }
    assert_ne!(
        first, second,
        "two invocations must not share a run id (2^-32 collision probability)"
    );
}

#[test]
fn namespace_ordinal_is_four_digits_and_bounded() {
    let minted = namespace::mint_namespace("112", "3f9a1c0e", 3).expect("ordinal 3 is in range");
    assert!(
        minted.contains("~0003~"),
        "ordinal must be four digits: {minted}"
    );

    assert!(
        namespace::mint_namespace("112", "3f9a1c0e", 0).is_err(),
        "ordinal 0 must be rejected"
    );
    assert!(
        namespace::mint_namespace("112", "3f9a1c0e", 10000).is_err(),
        "ordinal 10000 must be rejected"
    );
    assert!(
        namespace::check_discovery_cap(9999).is_ok(),
        "9999 documents are within the cap"
    );
    let err = namespace::check_discovery_cap(10000)
        .expect_err("10000 documents exceed the discovery cap");
    assert!(
        err.contains("10000"),
        "discovery-cap error must name the count: {err}"
    );
}

#[test]
fn namespace_rejects_duplicate_bindvar_or_base() {
    let duplicate_bind_var = [
        ("ns".to_string(), "112".to_string()),
        ("ns".to_string(), "113".to_string()),
    ];
    let err = namespace::validate_namespace_entries(&duplicate_bind_var, &[])
        .expect_err("duplicate bindVars must be rejected");
    assert!(
        err.contains("ns"),
        "error must name the offending bindVar: {err}"
    );

    let duplicate_base = [
        ("a".to_string(), "112".to_string()),
        ("b".to_string(), "112".to_string()),
    ];
    let err = namespace::validate_namespace_entries(&duplicate_base, &[])
        .expect_err("duplicate bases must be rejected");
    assert!(
        err.contains("112"),
        "error must name the offending base: {err}"
    );
}

#[test]
fn namespace_value_grammar_recheck() {
    assert!(
        namespace::is_valid_namespace_value("112~it~3f9a1c0e~0007~"),
        "the canonical mint must be accepted"
    );
    assert!(
        !namespace::is_valid_namespace_value("112~it~3f9a1c0e~7~"),
        "an unpadded ordinal must be rejected"
    );
    assert!(
        !namespace::is_valid_namespace_value("112~it~3f9a1c0e~0007"),
        "a missing trailing '~' must be rejected"
    );
    assert!(
        !namespace::is_valid_namespace_value(":112~it~3f9a1c0e~0007~"),
        "a non-alphanumeric base first char must be rejected"
    );
}
