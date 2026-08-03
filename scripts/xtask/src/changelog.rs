//! Generate categorized release notes from Conventional Commits.
//!
//! Parses `git log <range> --no-merges`, groups commits by type, detects
//! breaking changes, and emits Markdown suitable for a GitHub Release body.
//!
//! Breaking-change detection is warn-only: a commit whose body mentions
//! "breaking" but whose subject lacks the `!:` marker produces a stderr
//! warning rather than a non-zero exit. Rationale: enforce the convention
//! gently the first release, harden to `-D` once the team is trained.

use regex::Regex;
use std::process::Command;

/// Field separator inside a commit record (control char, unlikely in prose).
const FS: &str = "\x01";
/// Record separator between commits.
const RS: &str = "\x1e";

/// `(type, section_title)` pairs in display order. Maps a Conventional
/// Commit type to its heading in the release notes.
const CATEGORIES: &[(&str, &str)] = &[
    ("feat", "Features"),
    ("fix", "Fixes"),
    ("perf", "Performance"),
    ("refactor", "Refactor"),
    ("build", "Build"),
    ("docs", "Docs"),
];

/// Types excluded from user-facing release notes (internal / tooling).
const EXCLUDED_TYPES: &[&str] = &["chore", "test", "style", "ci", "revert", "plan", "spec"];

#[derive(Debug)]
struct Commit {
    hash: String,
    type_: String,
    scope: Option<String>,
    description: String,
    subject: String,
    body: String,
    breaking_subject: bool,
    breaking_body: bool,
}

impl Commit {
    fn is_breaking(&self) -> bool {
        self.breaking_subject || self.breaking_body
    }

    fn is_user_visible(&self) -> bool {
        !EXCLUDED_TYPES.contains(&self.type_.as_str())
    }
}

/// Entry point for `cargo xtask changelog`.
///
/// Writes categorized Markdown to stdout (intended for the GitHub Release
/// body) and diagnostics (warnings + SemVer recommendation) to stderr.
///
/// When `check` is true, runs as a CI gate: emits a single OK line on
/// stdout if no unmarked breaking changes are found, or returns an Err
/// (which the dispatcher turns into a non-zero exit) listing offenders.
pub fn run(from: Option<String>, to: Option<String>, check: bool) -> Result<(), String> {
    let start = resolve_from(from)?;
    let end = to.unwrap_or_else(|| "HEAD".to_string());
    let range = format!("{start}..{end}");

    let log = git_log(&range)?;
    let commits = parse_commits(&log);

    if check {
        return run_check(&commits, &range);
    }

    let body = render_body(&commits, &start);
    print!("{body}");

    emit_warnings(&commits);
    emit_bump_recommendation(&commits);

    Ok(())
}

/// CI gate mode: fail if any commit mentions breaking in prose without
/// the conventional `!:` subject marker (or a `BREAKING CHANGE:` body line).
fn run_check(commits: &[Commit], range: &str) -> Result<(), String> {
    let unmarked = find_unmarked_breaking(commits);
    if unmarked.is_empty() {
        println!(
            "lint-commits: OK ({} commits in {}, no unmarked breaking)",
            commits.len(),
            range
        );
        return Ok(());
    }
    let mut msg = format!(
        "lint-commits: FAILED — {} commit(s) mention breaking without the `!:` marker:\n",
        unmarked.len()
    );
    for c in unmarked {
        msg.push_str(&format!("  {} {}\n", short_hash(&c.hash), c.subject));
    }
    msg.push_str(
        "\nFix: add `!:` to the subject (e.g. `feat(scope)!: ...`) or add a \
         `BREAKING CHANGE:` footer to the body, or reword the body if the \
         word `breaking` describes a bug rather than a breaking change.",
    );
    Err(msg)
}

fn resolve_from(from: Option<String>) -> Result<String, String> {
    match from {
        Some(s) => Ok(s),
        None => last_core_tag(),
    }
}

/// Most recent `vX.Y.Z` tag, excluding bridge tags (`xml-bridge-v*`, etc.).
fn last_core_tag() -> Result<String, String> {
    let out = Command::new("git")
        .args(["tag", "--sort=-v:refname"])
        .output()
        .map_err(|e| format!("git tag: {e}"))?;
    if !out.status.success() {
        return Err("git tag failed".to_string());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .find(|t| is_core_tag(t))
        .map(String::from)
        .ok_or_else(|| "no core vX.Y.Z tag found; pass --from <tag-or-sha> explicitly".to_string())
}

fn is_core_tag(t: &str) -> bool {
    t.starts_with('v') && t.len() > 1 && t[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn git_log(range: &str) -> Result<String, String> {
    let format = format!("__REC__%H{FS}%s{FS}%b{RS}");
    let out = Command::new("git")
        .args(["log", "--no-merges", &format!("--format={format}"), range])
        .output()
        .map_err(|e| format!("git log: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git log failed: {err}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn parse_commits(log: &str) -> Vec<Commit> {
    let re =
        Regex::new(r"^(?P<type>[a-z]+)(?:\((?P<scope>[a-z0-9_-]+)\))?(?P<bang>!)?: (?P<desc>.+)$")
            .expect("static regex"); // allow-unwrap

    log.split(RS)
        .filter_map(|rec| {
            // git emits a trailing newline after each RS, so trim whitespace
            // BEFORE stripping the literal `__REC__` prefix.
            let rec = rec.trim().trim_start_matches("__REC__").trim();
            if rec.is_empty() {
                return None;
            }
            let mut parts = rec.splitn(3, FS);
            let hash = parts.next()?.trim().to_string();
            let subject = parts.next()?.trim().to_string();
            let body = parts
                .next()
                .map(|b| b.trim().to_string())
                .unwrap_or_default();

            let (type_, scope, breaking_subject, description) = match re.captures(&subject) {
                Some(caps) => {
                    let bang = caps.name("bang").is_some();
                    let t = caps
                        .name("type")
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default();
                    let s = caps.name("scope").map(|m| m.as_str().to_string());
                    let d = caps
                        .name("desc")
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_else(|| subject.clone());
                    (t, s, bang, d)
                }
                None => ("other".to_string(), None, false, subject.clone()),
            };

            Some(Commit {
                breaking_body: body.contains("BREAKING CHANGE:"),
                hash,
                type_,
                scope,
                description,
                subject,
                body,
                breaking_subject,
            })
        })
        .collect()
}

fn render_body(commits: &[Commit], from: &str) -> String {
    let mut out = String::new();
    out.push_str("## What's Changed\n\n");

    let breaking: Vec<&Commit> = commits.iter().filter(|c| c.is_breaking()).collect();
    if !breaking.is_empty() {
        out.push_str("### Breaking Changes\n\n");
        for c in &breaking {
            out.push_str(&format!("- {} ({})\n", c.description, short_hash(&c.hash)));
        }
        out.push('\n');
    }

    for (type_, title) in CATEGORIES {
        let bucket: Vec<&Commit> = commits
            .iter()
            .filter(|c| !c.is_breaking() && c.type_.as_str() == *type_)
            .collect();
        if bucket.is_empty() {
            continue;
        }
        out.push_str(&format!("### {title}\n\n"));
        for c in bucket {
            push_entry(&mut out, c);
        }
        out.push('\n');
    }

    // Unclassified but user-visible commits land here.
    let other: Vec<&Commit> = commits
        .iter()
        .filter(|c| {
            !c.is_breaking()
                && c.is_user_visible()
                && !CATEGORIES.iter().any(|(t, _)| *t == c.type_.as_str())
        })
        .collect();
    if !other.is_empty() {
        out.push_str("### Other Changes\n\n");
        for c in other {
            push_entry(&mut out, c);
        }
        out.push('\n');
    }

    let total = commits.len();
    out.push_str(&format!("---\n{total} commits since {from}.\n"));
    out
}

fn push_entry(out: &mut String, c: &Commit) {
    match &c.scope {
        Some(scope) => out.push_str(&format!(
            "- **{scope}**: {} ({})\n",
            c.description,
            short_hash(&c.hash)
        )),
        None => out.push_str(&format!("- {} ({})\n", c.description, short_hash(&c.hash))),
    }
}

fn short_hash(hash: &str) -> &str {
    if hash.len() >= 8 { &hash[..8] } else { hash }
}

/// Returns true if the body mentions breaking in prose, excluding the
/// standard `BREAKING CHANGE:` footer (handled separately via `marked`).
///
/// Heuristic (deliberately narrow to avoid false positives on bug
/// descriptions like "breaking body propagation"):
///   - phrase `breaking change` (case-insensitive)
///   - hyphenated form `[a-z]+-breaking` (e.g. `source-breaking`,
///     `api-breaking`, `semver-breaking`)
fn is_prose_breaking(body: &str) -> bool {
    let lower = body.to_lowercase();
    if lower.contains("breaking change") {
        return true;
    }
    let re = Regex::new(r"[a-z]+-breaking\b").expect("static regex"); // allow-unwrap
    re.is_match(&lower)
}

/// Commits whose bodies mention breaking in prose but lack both the `!:`
/// subject marker and the `BREAKING CHANGE:` body footer.
fn find_unmarked_breaking(commits: &[Commit]) -> Vec<&Commit> {
    commits
        .iter()
        .filter(|c| {
            let marked = c.breaking_subject || c.body.contains("BREAKING CHANGE:");
            !marked && is_prose_breaking(&c.body)
        })
        .collect()
}

fn emit_warnings(commits: &[Commit]) {
    let unmarked = find_unmarked_breaking(commits);
    if unmarked.is_empty() {
        return;
    }
    eprintln!();
    for c in unmarked {
        eprintln!(
            "warning: {} body mentions 'breaking' but subject lacks `!:` marker",
            short_hash(&c.hash)
        );
        eprintln!("         {}", c.subject);
    }
}

fn emit_bump_recommendation(commits: &[Commit]) {
    let has_breaking = commits.iter().any(|c| c.is_breaking());
    let has_feat = commits
        .iter()
        .any(|c| c.type_ == "feat" && !c.is_breaking());
    let fix_count = commits.iter().filter(|c| c.type_ == "fix").count();
    let perf_count = commits.iter().filter(|c| c.type_ == "perf").count();

    let recommendation = if has_breaking {
        "minor (breaking in 0.x)"
    } else if has_feat {
        "minor (new feature in 0.x)"
    } else if fix_count > 0 || perf_count > 0 {
        "patch"
    } else {
        "no bump needed"
    };

    eprintln!();
    eprintln!("SemVer (0.x) recommendation: {recommendation}");
    eprintln!("Counts: {}", format_counts(commits));
}

fn format_counts(commits: &[Commit]) -> String {
    let mut parts = Vec::new();
    let breaking = commits.iter().filter(|c| c.is_breaking()).count();
    if breaking > 0 {
        parts.push(format!("{breaking} breaking"));
    }
    for t in ["feat", "fix", "perf", "refactor", "docs", "build"] {
        let n = commits.iter().filter(|c| c.type_ == t).count();
        if n > 0 {
            parts.push(format!("{n} {t}"));
        }
    }
    if parts.is_empty() {
        "no user-visible changes".to_string()
    } else {
        parts.join(", ")
    }
}
