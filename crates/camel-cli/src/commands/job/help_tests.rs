//! Unit tests for the `camel job <name> --help` renderer.

use std::collections::BTreeMap;

use super::document::{JobArgumentDeclaration, JobArgumentDeclarations, JobHelpInfo};
use super::help::render_job_help;

/// The declared interface pinned by the full-render test: mode
/// `one-shot`, send target `direct:ingest`, and two declarations —
/// `feed` (required, described) and `region` (optional, defaulted) —
/// in lexical `BTreeMap` order.
fn declared_interface_info() -> JobHelpInfo {
    let mut entries = BTreeMap::new();
    entries.insert(
        "feed".to_string(),
        JobArgumentDeclaration {
            required: true,
            default: None,
            description: Some("Feed identifier".to_string()),
        },
    );
    entries.insert(
        "region".to_string(),
        JobArgumentDeclaration {
            required: false,
            default: Some("eu-west-1".to_string()),
            description: None,
        },
    );
    JobHelpInfo {
        mode: "one-shot".to_string(),
        send_to: "direct:ingest".to_string(),
        args: Some(JobArgumentDeclarations { entries }),
    }
}

#[test]
fn render_full_declared_interface() {
    let info = declared_interface_info();
    let rendered = render_job_help("daily-sync", Some("Ingest the daily feed"), &info);
    let expected = "\
daily-sync

Ingest the daily feed

Mode:      one-shot
Sends to:  direct:ingest

Arguments:
  feed    string  required  Feed identifier
  region  string  optional  default=eu-west-1";
    assert_eq!(rendered, expected);
}

#[test]
fn render_no_description_placeholder() {
    let info = declared_interface_info();
    let rendered = render_job_help("daily-sync", None, &info);
    let line = rendered.lines().nth(2).expect("line 3 exists");
    assert_eq!(line, "(no description)");
}

#[test]
fn render_absent_and_empty_args_print_no_arguments() {
    let absent = JobHelpInfo {
        mode: "one-shot".to_string(),
        send_to: "direct:ingest".to_string(),
        args: None,
    };
    let empty = JobHelpInfo {
        mode: "one-shot".to_string(),
        send_to: "direct:ingest".to_string(),
        args: Some(JobArgumentDeclarations {
            entries: BTreeMap::new(),
        }),
    };
    let rendered_absent = render_job_help("daily-sync", None, &absent);
    let rendered_empty = render_job_help("daily-sync", None, &empty);
    assert_eq!(rendered_absent, rendered_empty);
    let mut lines = rendered_absent
        .lines()
        .skip_while(|line| *line != "Arguments:");
    assert_eq!(lines.next(), Some("Arguments:"));
    assert_eq!(lines.next(), Some("  (no arguments)"));
    assert_eq!(lines.next(), None);
}

#[test]
fn render_multiline_values_collapse_to_one_row() {
    let mut entries = BTreeMap::new();
    entries.insert(
        "payload".to_string(),
        JobArgumentDeclaration {
            required: false,
            default: Some("a\nb".to_string()),
            description: Some("c\r\nd".to_string()),
        },
    );
    let info = JobHelpInfo {
        mode: "one-shot".to_string(),
        send_to: "direct:ingest".to_string(),
        args: Some(JobArgumentDeclarations { entries }),
    };
    let rendered = render_job_help("daily-sync", None, &info);
    let row = rendered.lines().last().expect("argument row exists");
    assert!(!row.contains('\r'), "row must not contain CR: {row}");
    assert!(!row.contains('\n'), "row must not contain LF: {row}");
    assert!(row.contains("default=a b"), "row: {row}");
    assert!(row.contains("c d"), "row: {row}");
}

#[test]
fn render_multiline_description_collapses_to_one_line() {
    let info = JobHelpInfo {
        mode: "one-shot".to_string(),
        send_to: "direct:ingest".to_string(),
        args: None,
    };
    let rendered = render_job_help("daily-sync", Some("Daily feed\r\ningest"), &info);
    let description_line = rendered.lines().nth(2).expect("description line exists");
    assert_eq!(description_line, "Daily feed ingest");
    assert!(
        !description_line.contains('\r'),
        "description line must not contain CR: {description_line}"
    );
    assert!(
        !description_line.contains('\n'),
        "description line must not contain LF: {description_line}"
    );
    // Layout is unchanged: `Mode:` stays on line 5 (stem, blank,
    // description, blank, Mode:).
    assert_eq!(
        rendered.lines().nth(4),
        Some("Mode:      one-shot"),
        "Mode: line must stay at line 5"
    );
}

#[test]
fn render_token_send_target_verbatim() {
    let info = JobHelpInfo {
        mode: "one-shot".to_string(),
        send_to: "${arg:target}".to_string(),
        args: None,
    };
    let rendered = render_job_help("daily-sync", None, &info);
    let line = rendered
        .lines()
        .find(|line| line.starts_with("Sends to:"))
        .expect("Sends to: line exists");
    assert_eq!(line, "Sends to:  ${arg:target}");
}
