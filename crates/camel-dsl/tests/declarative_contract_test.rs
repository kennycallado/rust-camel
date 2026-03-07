use camel_dsl::{DeclarativeStepKind, is_rust_only_kind, mandatory_declarative_step_kinds};

#[test]
fn mandatory_contract_marks_only_closure_based_features_as_rust_only() {
    assert!(!is_rust_only_kind(DeclarativeStepKind::To));
    assert!(!is_rust_only_kind(DeclarativeStepKind::Script));
    assert!(is_rust_only_kind(DeclarativeStepKind::Process));
    assert!(is_rust_only_kind(DeclarativeStepKind::SetHeaderFn));
}

#[test]
fn mandatory_contract_lists_script_as_required_dsl_support() {
    let kinds = mandatory_declarative_step_kinds();
    assert!(kinds.contains(&DeclarativeStepKind::Script));
    assert!(!kinds.contains(&DeclarativeStepKind::Process));
}
