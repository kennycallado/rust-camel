//! Declared case labels and per-document accounting (test-count-honesty-b D1, D2, D5).

use super::document::ParsedDocument;
use super::runner::EndpointResult;

/// Declared case labels a successful run prints, in print order.
pub(crate) fn declared_labels(doc: &ParsedDocument) -> Vec<String> {
    match doc {
        ParsedDocument::Unit(unit) => {
            let mut labels: Vec<String> = unit.expects.keys().cloned().collect();
            for (index, input) in unit.inputs.iter().enumerate() {
                if input.expect_reply.is_some() {
                    labels.push(format!("reply[{index}] {}", input.to));
                }
            }
            if unit.sequence.is_some() {
                labels.push("<sequence>".to_string());
            }
            labels
        }
        ParsedDocument::Scenario(scenario) => scenario
            .scenario
            .iter()
            .enumerate()
            .map(|(index, action)| super::scenario::action_label(index, action))
            .collect(),
    }
}

/// Stdout `not run` deficit for one document: declared labels without a
/// produced verdict row. Every produced row counts, pass or fail, including
/// the `<settle>` row (D2), so rows beyond the declared set saturate at zero.
pub(crate) fn not_run_deficit(declared: &[String], rows: &[EndpointResult]) -> usize {
    declared.len().saturating_sub(rows.len())
}

/// Declared labels with no produced row, paired with their skip reason (D5
/// precedence: document error, then settle timeout, then the first failed
/// scenario action). Entries render in declared order.
pub(crate) fn skipped_cases<'a>(
    declared: &'a [String],
    rows: &[EndpointResult],
    doc_error: bool,
) -> Vec<(&'a str, String)> {
    let reason = if doc_error {
        "document error".to_string()
    } else if rows.iter().any(|row| row.endpoint == "<settle>") {
        "settle timeout".to_string()
    } else if let Some(index) = first_failed_scenario_index(rows) {
        format!("stopped after scenario[{index}] failure")
    } else {
        // Unreachable for evaluator output: a partial row set always carries
        // a failure. Keeps the function total.
        "document error".to_string()
    };
    declared
        .iter()
        .filter(|label| !rows.iter().any(|row| row.endpoint == label.as_str()))
        .map(|label| (label.as_str(), reason.clone()))
        .collect()
}

/// Scenario index parsed out of the first failing `scenario[{i}] …` row.
fn first_failed_scenario_index(rows: &[EndpointResult]) -> Option<usize> {
    rows.iter()
        .filter(|row| row.outcome.is_err())
        .find_map(|row| {
            row.endpoint
                .strip_prefix("scenario[")
                .and_then(|rest| rest.split_once(']'))
                .and_then(|(index, _)| index.parse().ok())
        })
}

#[cfg(test)]
#[path = "declared_tests.rs"]
mod declared_tests;
