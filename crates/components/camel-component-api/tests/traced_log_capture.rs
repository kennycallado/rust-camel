use camel_component_api::test_support::log_capture::traced::{
    captured_records_containing, clear_captured_logs, lock_log_capture,
};
use tracing_test::traced_test;

#[traced_test]
#[test]
fn traced_records_containing_requires_all_needles_on_one_line() {
    let _guard = lock_log_capture();
    clear_captured_logs();

    tracing::warn!("epoch regression prior_epoch=7");
    tracing::info!("unrelated line");

    let records = captured_records_containing(&["epoch", "regression"]);
    assert_eq!(records.len(), 1, "expected exactly one matching record");
    assert!(
        records[0].contains("prior_epoch=7"),
        "missing prior_epoch=7 field: {}",
        records[0]
    );
    assert!(
        captured_records_containing(&["epoch", "unrelated"]).is_empty(),
        "needles must all match on the same line"
    );
    assert_eq!(
        captured_records_containing(&[" WARN "]).len(),
        1,
        "level pin contract: captured line must carry the WARN level"
    );
}

#[traced_test]
#[test]
fn traced_clear_then_emit_starts_clean() {
    let _guard = lock_log_capture();
    tracing::warn!("pre-clear noise");
    clear_captured_logs();

    assert!(
        captured_records_containing(&["pre-clear"]).is_empty(),
        "clear must drop previously captured records"
    );

    tracing::warn!("post-clear ping");
    assert_eq!(
        captured_records_containing(&["post-clear"]).len(),
        1,
        "expected exactly one post-clear record"
    );
}
