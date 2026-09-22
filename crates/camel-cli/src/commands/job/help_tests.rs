//! Unit tests for the `camel job <name> --help` renderer.

use std::collections::BTreeMap;
use std::path::Path;

use super::document::{
    JobArgType, JobArgumentDeclaration, JobArgumentDeclarations, JobDocError, JobHelpInfo,
    parse_job_document_for_help,
};
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
            arg_type: JobArgType::String,
        },
    );
    entries.insert(
        "region".to_string(),
        JobArgumentDeclaration {
            required: false,
            default: Some("eu-west-1".to_string()),
            description: None,
            arg_type: JobArgType::String,
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

/// The renderer accepts ANY header string and prints it verbatim as
/// line 1 — including a nested invocable path. This pins the renderer
/// contract; the call-site WIRING (which passes the display name) is
/// proven by the `nested_job_help_header_shows_invocable_path`
/// integration test.
#[test]
fn render_nested_display_name_verbatim() {
    let info = declared_interface_info();
    let rendered = render_job_help("daily/ingest.job.yaml", Some("Ingest"), &info);
    let header = rendered.lines().next().expect("header line exists");
    assert_eq!(header, "daily/ingest.job.yaml");
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
            arg_type: JobArgType::String,
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

/// A declaration with only its type set (optional, no default, no
/// description).
fn typed_declaration(arg_type: JobArgType) -> JobArgumentDeclaration {
    JobArgumentDeclaration {
        required: false,
        default: None,
        description: None,
        arg_type,
    }
}

/// The `required`/`optional` marker's column index on the row of
/// `name`, or the row's length when no marker is present.
fn marker_index(rendered: &str, name: &str) -> usize {
    let row = rendered
        .lines()
        .find(|line| line.starts_with(&format!("  {name} ")))
        .unwrap_or_else(|| panic!("row for `{name}` exists"));
    row.find("required")
        .or_else(|| row.find("optional"))
        .unwrap_or_else(|| panic!("row for `{name}` carries a marker: {row}"))
}

#[test]
fn help_renders_declared_type_per_argument() {
    let mut entries = BTreeMap::new();
    entries.insert("count".to_string(), typed_declaration(JobArgType::Int));
    entries.insert("name".to_string(), typed_declaration(JobArgType::String));
    entries.insert(
        "tier".to_string(),
        typed_declaration(JobArgType::Enum(vec![
            "bronze".to_string(),
            "gold".to_string(),
        ])),
    );
    entries.insert("verbose".to_string(), typed_declaration(JobArgType::Bool));
    let info = JobHelpInfo {
        mode: "one-shot".to_string(),
        send_to: "direct:ingest".to_string(),
        args: Some(JobArgumentDeclarations { entries }),
    };
    let rendered = render_job_help("daily-sync", None, &info);
    let row = |name: &str| {
        rendered
            .lines()
            .find(|line| line.starts_with(&format!("  {name} ")))
            .unwrap_or_else(|| panic!("row for `{name}` exists"))
    };
    assert!(row("count").contains("int"), "count row: {}", row("count"));
    assert!(
        row("verbose").contains("bool"),
        "verbose row: {}",
        row("verbose")
    );
    assert!(
        row("tier").contains("enum[bronze,gold]"),
        "tier row: {}",
        row("tier")
    );
    assert!(row("name").contains("string"), "name row: {}", row("name"));
}

#[test]
fn help_type_column_aligns_across_rows() {
    let mut mixed_entries = BTreeMap::new();
    mixed_entries.insert("count".to_string(), typed_declaration(JobArgType::Int));
    mixed_entries.insert(
        "tier".to_string(),
        typed_declaration(JobArgType::Enum(vec![
            "bronze".to_string(),
            "gold".to_string(),
        ])),
    );
    let mixed = JobHelpInfo {
        mode: "one-shot".to_string(),
        send_to: "direct:ingest".to_string(),
        args: Some(JobArgumentDeclarations {
            entries: mixed_entries,
        }),
    };
    let rendered_mixed = render_job_help("daily-sync", None, &mixed);
    // The widest type (`enum[bronze,gold]`) sets the column, so both
    // markers start at the same index.
    assert_eq!(
        marker_index(&rendered_mixed, "count"),
        marker_index(&rendered_mixed, "tier")
    );
    // A job where every row IS the widest type aligns identically.
    let mut widest_entries = BTreeMap::new();
    widest_entries.insert(
        "count".to_string(),
        typed_declaration(JobArgType::Enum(vec![
            "bronze".to_string(),
            "gold".to_string(),
        ])),
    );
    widest_entries.insert(
        "tier".to_string(),
        typed_declaration(JobArgType::Enum(vec![
            "bronze".to_string(),
            "gold".to_string(),
        ])),
    );
    let widest = JobHelpInfo {
        mode: "one-shot".to_string(),
        send_to: "direct:ingest".to_string(),
        args: Some(JobArgumentDeclarations {
            entries: widest_entries,
        }),
    };
    let rendered_widest = render_job_help("daily-sync", None, &widest);
    assert_eq!(
        marker_index(&rendered_mixed, "count"),
        marker_index(&rendered_widest, "count")
    );
}

#[test]
fn help_untyped_job_renders_string_column_unchanged() {
    // The A3 golden declarations: `feed` (required, described) and
    // `region` (optional, defaulted) — the same set the full-render
    // golden test pins. Untyped declarations render `string` as both
    // the widest and only type, so the output is byte-identical to the
    // pre-change renderer.
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
fn help_declaration_error_exits_2() {
    // Help shares the declaration checks with execution parsing: a
    // typed default failing coercion fails the help parse with the same
    // error class.
    let text = "\
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:in
routeFiles:
  - routes.yaml
args:
  count:
    type: int
    default: \"abc\"";
    let err = parse_job_document_for_help(Path::new("daily.job.yaml"), text)
        .expect_err("typed default failing coercion must fail the help parse");
    match err {
        JobDocError::ArgumentCoercion {
            name,
            expected,
            raw,
        } => {
            assert_eq!(name, "count");
            assert_eq!(expected, JobArgType::Int);
            assert_eq!(raw, "abc");
        }
        other => panic!("expected ArgumentCoercion, got {other:?}"),
    }
}
