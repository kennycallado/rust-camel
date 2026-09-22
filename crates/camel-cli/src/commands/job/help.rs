//! Renderer for `camel job <name> --help`: a pure projection of the
//! document's declared interface into its help text. No I/O — the
//! caller parses the document
//! ([`super::document::parse_job_document_for_help`]), probes the
//! description, and prints the returned text.

use super::document::{JobArgType, JobArgumentDeclaration, JobHelpInfo};

/// Render the declared interface of one job document as its
/// `camel job <name> --help` text. Pure: no I/O, no process state; the
/// result is `\n`-separated lines without a trailing newline (the
/// caller prints with `println!`).
///
/// The first parameter is the job's display name — the invocable
/// spelling: the configured-root-relative path for nested documents,
/// the file stem otherwise — rendered verbatim as the header line.
///
/// Layout, in order: the display name; a blank line; the description
/// (or `(no description)`); a blank line; the aligned `Mode:`/`Sends
/// to:` pair (both values start at column 12); a blank line;
/// `Arguments:`; then one row per declared argument in lexical order,
/// or the single row `  (no arguments)` when nothing is declared.
/// CR/LF runs inside the description, default values, and argument
/// descriptions flatten to one space so every help line stays one
/// physical line.
pub(crate) fn render_job_help(
    display_name: &str,
    description: Option<&str>,
    info: &JobHelpInfo,
) -> String {
    let mut out = String::new();
    out.push_str(display_name);
    out.push_str("\n\n");
    match description {
        Some(description) => out.push_str(&flatten_line_breaks(description)),
        None => out.push_str("(no description)"),
    }
    out.push_str("\n\n");
    // Label alignment: `Mode:` + 6 trailing spaces and `Sends to:` +
    // 2 — both value columns start at column 12.
    out.push_str("Mode:      ");
    out.push_str(&info.mode);
    out.push('\n');
    out.push_str("Sends to:  ");
    out.push_str(&info.send_to);
    out.push_str("\n\n");
    out.push_str("Arguments:");
    match &info.args {
        Some(declarations) if !declarations.entries.is_empty() => {
            // Name column width: the widest declared name; names are
            // validated ASCII identifiers, so byte length is the
            // display width.
            let name_width = declarations
                .entries
                .keys()
                .map(String::len)
                .max()
                .unwrap_or_default();
            // Type column width: the widest rendered type in this job,
            // padded by byte length — the same strategy as the name
            // column. Enum members are not grammar-restricted to
            // ASCII, so exotic members may visually misalign; tighten
            // the `enum[...]` member grammar if that ever matters.
            let type_width = declarations
                .entries
                .values()
                .map(|declaration| JobArgType::render(&declaration.arg_type).len())
                .max()
                .unwrap_or_default();
            for (name, declaration) in &declarations.entries {
                out.push('\n');
                out.push_str(&render_argument_row(
                    name,
                    name_width,
                    type_width,
                    declaration,
                ));
            }
        }
        _ => out.push_str("\n  (no arguments)"),
    }
    out
}

/// Render one argument row: two leading spaces, the name padded on the
/// right to the table's name column, the declared type rendered by
/// [`JobArgType::render`] and padded on the right to the table's type
/// column, the requirement flag, then the `  default=<value>` and
/// `  <description>` suffixes when declared.
fn render_argument_row(
    name: &str,
    name_width: usize,
    type_width: usize,
    declaration: &JobArgumentDeclaration,
) -> String {
    let mut row = String::new();
    row.push_str("  ");
    row.push_str(name);
    row.push_str(&" ".repeat(name_width - name.len()));
    row.push_str("  ");
    let rendered_type = declaration.arg_type.render();
    row.push_str(&rendered_type);
    row.push_str(&" ".repeat(type_width - rendered_type.len()));
    row.push_str("  ");
    row.push_str(if declaration.required {
        "required"
    } else {
        "optional"
    });
    if let Some(default) = &declaration.default {
        row.push_str("  default=");
        row.push_str(&flatten_line_breaks(default));
    }
    if let Some(description) = &declaration.description {
        row.push_str("  ");
        row.push_str(&flatten_line_breaks(description));
    }
    row
}

/// Flatten CR/LF runs in a declared value to single spaces so a
/// multi-line `default` or `description` cannot break the argument's
/// single row: each maximal run of `\r`/`\n` characters becomes one
/// space (`a\nb` renders `a b`, `c\r\nd` renders `c d`).
fn flatten_line_breaks(text: &str) -> String {
    let mut flat = String::with_capacity(text.len());
    let mut in_break = false;
    for ch in text.chars() {
        if ch == '\r' || ch == '\n' {
            if !in_break {
                flat.push(' ');
                in_break = true;
            }
        } else {
            flat.push(ch);
            in_break = false;
        }
    }
    flat
}
