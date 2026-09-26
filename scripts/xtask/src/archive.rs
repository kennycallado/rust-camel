//! `openspec archive` wrapper: scenario rename/drop directives (archengine).
//!
//! The upstream CLI replaces a delta's MODIFIED requirement block wholesale
//! and refuses when the canonical block still holds scenarios the delta no
//! longer lists, so renaming or dropping a scenario has no first-class
//! syntax. A delta block may instead carry an HTML directive comment:
//!
//! ```text
//! <!-- openspec-scenario-ops
//! renamed: Old scenario -> New scenario
//! dropped: Some scenario | one-line justification (required)
//! -->
//! ```
//!
//! This module implements the directive parser, the guard matrix
//! (rules R1-R10 in `openspec/changes/archengine/design.md`), canon splice,
//! delta discovery, and the `cargo xtask archive` wrapper flow.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;
use walkdir::WalkDir;

/// A directive-parser or guard violation, tagged with the design rule that
/// produced it (`R1`..`R10` in `openspec/changes/archengine/design.md`).
#[derive(Debug, Clone)]
pub struct ArchiveError {
    /// Design rule id, e.g. `"R3"`.
    pub rule: &'static str,
    /// Requirement the violation is about, when known.
    pub requirement: Option<String>,
    /// Scenario the violation is about, when known.
    pub scenario: Option<String>,
    /// Concrete problem, worded for conductor consumption.
    pub problem: String,
}

impl ArchiveError {
    fn new(rule: &'static str, problem: impl Into<String>) -> Self {
        Self {
            rule,
            requirement: None,
            scenario: None,
            problem: problem.into(),
        }
    }

    fn with_requirement(mut self, requirement: impl Into<String>) -> Self {
        self.requirement = Some(requirement.into());
        self
    }

    fn with_scenario(mut self, scenario: impl Into<String>) -> Self {
        self.scenario = Some(scenario.into());
        self
    }
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.rule)?;
        if let Some(requirement) = &self.requirement {
            write!(f, " requirement {requirement:?}")?;
        }
        if let Some(scenario) = &self.scenario {
            write!(f, " scenario {scenario:?}")?;
        }
        write!(f, ": {}", self.problem)
    }
}

impl std::error::Error for ArchiveError {}

/// One scenario-level operation declared by a directive comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenarioOp {
    /// `renamed: <from> -> <to>`
    Renamed {
        /// Scenario name in the canonical block.
        from: String,
        /// Scenario name in the delta block.
        to: String,
    },
    /// `dropped: <name> | <justification>`
    Dropped {
        /// Scenario name removed by the delta.
        name: String,
        /// Required one-line rationale; empty justifications are rejected.
        justification: String,
    },
}

/// Parsed `openspec-scenario-ops` directive comment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DirectiveComment {
    /// Operations in declaration order.
    pub ops: Vec<ScenarioOp>,
}

/// Marker that opens a directive comment span.
const DIRECTIVE_OPEN: &str = "<!-- openspec-scenario-ops";
/// Marker that closes a directive comment span.
const DIRECTIVE_CLOSE: &str = "-->";

/// Scan a requirement block's raw lines for the `openspec-scenario-ops`
/// directive comment. `Ok(None)` when the block carries no directive;
/// a second comment span, an unterminated span, or any malformed directive
/// line is a rule-tagged error. Fence-masked like every other extractor:
/// a directive comment shown inside a fenced code example is body text,
/// not an op.
pub fn parse_directive_comment(
    block_lines: &[String],
) -> Result<Option<DirectiveComment>, ArchiveError> {
    let str_lines: Vec<&str> = block_lines.iter().map(String::as_str).collect();
    let mask = build_code_fence_mask(&str_lines);
    let mut seen_open = false;
    let mut inside = false;
    let mut inner: Vec<&str> = Vec::new();
    for (index, line) in block_lines.iter().enumerate() {
        if mask[index] {
            continue;
        }
        let trimmed = line.trim();
        if inside {
            if trimmed.starts_with(DIRECTIVE_CLOSE) {
                inside = false;
            } else if !trimmed.is_empty() {
                inner.push(trimmed);
            }
        } else if trimmed.starts_with(DIRECTIVE_OPEN) {
            if seen_open {
                return Err(ArchiveError::new(
                    "R8",
                    "second `openspec-scenario-ops` comment in one requirement block; at most one is allowed",
                ));
            }
            seen_open = true;
            inside = true;
        }
    }
    if inside {
        return Err(ArchiveError::new(
            "R8",
            "`openspec-scenario-ops` comment is never closed with `-->`",
        ));
    }
    if !seen_open {
        return Ok(None);
    }
    let mut ops = Vec::new();
    for line in inner {
        parse_directive_line(line, &mut ops)?;
    }
    if ops.is_empty() {
        return Err(ArchiveError::new(
            "R8",
            "`openspec-scenario-ops` comment declares no operations",
        ));
    }
    Ok(Some(DirectiveComment { ops }))
}

/// Parse one directive-comment inner line into `ops`.
fn parse_directive_line(line: &str, ops: &mut Vec<ScenarioOp>) -> Result<(), ArchiveError> {
    check_forbidden_content(line)?;
    if let Some(rest) = line.strip_prefix("renamed:") {
        let (from, to) = split_rename(rest)?;
        ops.push(ScenarioOp::Renamed { from, to });
        return Ok(());
    }
    if let Some(rest) = line.strip_prefix("dropped:") {
        let (name, justification) = split_drop(rest)?;
        ops.push(ScenarioOp::Dropped {
            name,
            justification,
        });
        return Ok(());
    }
    Err(ArchiveError::new(
        "R8",
        format!(
            "unknown directive line {line:?}; only `renamed: <from> -> <to>` and \
             `dropped: <name> | <justification>` are allowed"
        ),
    ))
}

/// R10 content invariants: a directive line must stay invisible to the
/// upstream delta validator, so it never names a scenario header nor uses
/// the words SHALL/MUST.
fn check_forbidden_content(line: &str) -> Result<(), ArchiveError> {
    if line.contains("#### Scenario:") {
        return Err(ArchiveError::new(
            "R10",
            "directive line contains `#### Scenario:`; directive content must stay \
             invisible to the delta validator",
        ));
    }
    if shall_must_re().is_match(line) {
        return Err(ArchiveError::new(
            "R10",
            "directive line contains SHALL or MUST; directive content must stay \
             invisible to the delta validator",
        ));
    }
    Ok(())
}

/// Split a `renamed:` value into `(from, to)` (R10 delimiter rules).
fn split_rename(rest: &str) -> Result<(String, String), ArchiveError> {
    let arrows = rest.matches("->").count();
    if arrows != 1 {
        return Err(ArchiveError::new(
            "R10",
            format!("`renamed:` line needs exactly one `->` separator, found {arrows}: {rest:?}"),
        ));
    }
    let Some((from, to)) = rest.split_once("->") else {
        return Err(ArchiveError::new(
            "R10",
            "`renamed:` line is missing its `->` separator",
        ));
    };
    Ok((checked_name(from.trim())?, checked_name(to.trim())?))
}

/// Split a `dropped:` value into `(name, justification)` (R3/R10 rules).
fn split_drop(rest: &str) -> Result<(String, String), ArchiveError> {
    let Some((name, justification)) = rest.split_once('|') else {
        return Err(ArchiveError::new(
            "R3",
            format!("`dropped:` line needs a `| <justification>` suffix, got {rest:?}"),
        ));
    };
    let name = checked_name(name.trim())?;
    let justification = justification.trim();
    if justification.is_empty() {
        return Err(ArchiveError::new(
            "R3",
            format!("drop of scenario {name:?} requires a non-empty justification"),
        ));
    }
    Ok((name, justification.to_owned()))
}

/// Validate a scenario name: non-empty, no `->`/`|` delimiter collisions.
fn checked_name(name: &str) -> Result<String, ArchiveError> {
    if name.is_empty() {
        return Err(ArchiveError::new("R10", "scenario name is empty"));
    }
    if name.contains("->") || name.contains('|') {
        return Err(ArchiveError::new(
            "R10",
            format!("scenario name {name:?} must not contain `->` or `|`"),
        ));
    }
    Ok(name.to_owned())
}

/// One `### Requirement:` block of a spec file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequirementBlock {
    /// Trimmed requirement header text.
    pub name: String,
    /// Raw block: header line plus body, `\n`-joined and `trim_end`ed.
    pub raw: String,
}

/// Split a spec file into its `### Requirement:` blocks, porting upstream
/// `extractRequirementsSection` boundary semantics: the `## Requirements`
/// section (case-insensitive, never inside a code fence) ends at the next
/// `## ` heading, and a block runs from its header to the next requirement
/// or `## ` heading. A fenced `### Requirement:` line is body text, not a
/// boundary.
pub fn split_requirement_blocks(spec_md: &str) -> Vec<RequirementBlock> {
    requirement_blocks_with_ranges(spec_md, requirements_section_re())
        .into_iter()
        .map(|(block, _, _)| block)
        .collect()
}

/// `split_requirement_blocks` over an arbitrary section-heading pattern,
/// keeping each block's line range (start, end-exclusive) in the normalized
/// line vector. Used for canon (`## Requirements`) and delta
/// (`## MODIFIED Requirements`) files alike.
fn requirement_blocks_with_ranges(
    spec_md: &str,
    section_re: &Regex,
) -> Vec<(RequirementBlock, usize, usize)> {
    let normalized = normalize_line_endings(spec_md);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mask = build_code_fence_mask(&lines);
    let Some((section_start, section_end)) = find_section_bounds(&lines, &mask, section_re) else {
        return Vec::new();
    };
    block_ranges(&lines, &mask, section_start, section_end)
        .into_iter()
        .map(|(name, start, end)| {
            let raw = lines[start..end].join("\n").trim_end().to_owned();
            (RequirementBlock { name, raw }, start, end)
        })
        .collect()
}

/// Locate a `section_re` H2 heading and its end (next unfenced `## `
/// heading, or end of file). Returns `(start, end)` line indices, the start
/// pointing at the heading itself.
fn find_section_bounds(
    lines: &[&str],
    mask: &[bool],
    section_re: &Regex,
) -> Option<(usize, usize)> {
    let section_start = lines
        .iter()
        .zip(mask)
        .position(|(line, fenced)| !fenced && section_re.is_match(line))?;
    let mut section_end = lines.len();
    for (index, line) in lines.iter().enumerate().skip(section_start + 1) {
        if !mask[index] && is_h2_heading(line) {
            section_end = index;
            break;
        }
    }
    Some((section_start, section_end))
}

/// Requirement blocks of one section as `(name, start, end-exclusive)` line
/// ranges, fence-aware: a masked `### Requirement:` line is body text.
fn block_ranges(
    lines: &[&str],
    mask: &[bool],
    section_start: usize,
    section_end: usize,
) -> Vec<(String, usize, usize)> {
    let mut ranges = Vec::new();
    let mut cursor = section_start + 1;
    while cursor < section_end {
        if mask[cursor] {
            cursor += 1;
            continue;
        }
        let Some(name) = requirement_header_name(lines[cursor]) else {
            cursor += 1;
            continue;
        };
        let start = cursor;
        cursor += 1;
        while cursor < section_end
            && (mask[cursor]
                || (requirement_header_name(lines[cursor]).is_none()
                    && !is_h2_heading(lines[cursor])))
        {
            cursor += 1;
        }
        ranges.push((name, start, cursor));
    }
    ranges
}

/// Scenario names declared by `#### Scenario:` headers in a block, in
/// order, duplicates preserved (multiplicity matters to the guards).
/// Fence-masked: a scenario header inside a code sample is body text.
pub fn scenario_names(block_raw: &str) -> Vec<String> {
    let lines: Vec<&str> = block_raw.split('\n').collect();
    let mask = build_code_fence_mask(&lines);
    let mut names = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !mask[index]
            && let Some(name) = scenario_header_name(line)
        {
            names.push(name);
        }
    }
    names
}

/// `true` when the line is an `## ` heading (any H2, upstream `/^##\s+/`).
fn is_h2_heading(line: &str) -> bool {
    match line.strip_prefix("##") {
        Some(rest) => rest.starts_with(char::is_whitespace),
        None => false,
    }
}

/// `\r\n` and stray `\r` normalized to `\n`, as upstream does on entry.
fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

#[derive(Debug, Clone, Copy)]
struct ActiveFence {
    marker: u8,
    length: usize,
}

/// `true` marks a line inside a fenced code block, opener and closer
/// included. Port of upstream `buildCodeFenceMask`: a fence opens with
/// 3+ backticks or tildes (leading whitespace and an info string allowed)
/// and closes with 3+ of the same marker alone on the line.
fn build_code_fence_mask(lines: &[&str]) -> Vec<bool> {
    let mut mask = vec![false; lines.len()];
    let mut active: Option<ActiveFence> = None;
    for (index, line) in lines.iter().enumerate() {
        match active {
            None => {
                if let Some(fence) = opening_fence(line) {
                    active = Some(fence);
                    mask[index] = true;
                }
            }
            Some(fence) => {
                mask[index] = true;
                if is_closing_fence(line, fence) {
                    active = None;
                }
            }
        }
    }
    mask
}

fn opening_fence(line: &str) -> Option<ActiveFence> {
    let bytes = line.trim_start().as_bytes();
    let marker = *bytes.first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let length = bytes.iter().take_while(|&&byte| byte == marker).count();
    (length >= 3).then_some(ActiveFence { marker, length })
}

fn is_closing_fence(line: &str, fence: ActiveFence) -> bool {
    let trimmed = line.trim_start();
    let bytes = trimmed.as_bytes();
    let marker = match bytes.first() {
        Some(&marker) if marker == b'`' || marker == b'~' => marker,
        _ => return false,
    };
    if marker != fence.marker {
        return false;
    }
    let length = bytes.iter().take_while(|&&byte| byte == marker).count();
    length >= fence.length && trimmed[length..].trim().is_empty()
}

/// Requirement header text (`### Requirement: <name>`), case-insensitive,
/// or `None` when the line is not a requirement header.
fn requirement_header_name(line: &str) -> Option<String> {
    requirement_header_re()
        .captures(line)
        .and_then(|captures| captures.get(1))
        .map(|name| name.as_str().trim().to_owned())
}

/// Scenario header text (`#### Scenario: <name>`), case-insensitive.
fn scenario_header_name(line: &str) -> Option<String> {
    scenario_header_re()
        .captures(line)
        .and_then(|captures| captures.get(1))
        .map(|name| name.as_str().trim().to_owned())
}

/// Compile a built-in pattern; a broken constant is a programmer error.
fn compile(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(error) => panic!("built-in regex {pattern:?} failed to compile: {error}"),
    }
}

fn shall_must_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| compile(r"(?i)\b(?:SHALL|MUST)\b"))
}

fn requirements_section_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| compile(r"(?i)^##\s+Requirements\s*$"))
}

fn requirement_header_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| compile(r"(?i)^###\s*Requirement:\s*(.+?)\s*$"))
}

fn scenario_header_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| compile(r"(?i)^####\s*Scenario:\s*(.+?)\s*$"))
}

fn blank_run_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| compile(r"\n{3,}"))
}

fn modified_requirements_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| compile(r"(?i)^##\s+modified\s+requirements\s*$"))
}

fn renamed_section_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| compile(r"(?i)^##\s+renamed\b"))
}

/// Unfenced directive-comment opener lines living outside every requirement
/// block of the file — the R8 structural violation.
fn directive_lines_outside_blocks(spec_md: &str, ranges: &[(usize, usize)]) -> Vec<usize> {
    let normalized = normalize_line_endings(spec_md);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mask = build_code_fence_mask(&lines);
    lines
        .iter()
        .enumerate()
        .filter(|(index, line)| {
            !mask[*index]
                && line.trim().starts_with(DIRECTIVE_OPEN)
                && !ranges
                    .iter()
                    .any(|(start, end)| *index >= *start && *index < *end)
        })
        .map(|(index, _)| index)
        .collect()
}

/// `true` when the block carries an unfenced `openspec-scenario-ops`
/// opener line. Fence-masked to match `parse_directive_comment`: a
/// directive comment quoted inside a code example is body text, not an op.
fn has_unfenced_directive_open(block_raw: &str) -> bool {
    let lines: Vec<&str> = block_raw.split('\n').collect();
    let mask = build_code_fence_mask(&lines);
    lines
        .iter()
        .zip(&mask)
        .any(|(line, fenced)| !fenced && line.trim().starts_with(DIRECTIVE_OPEN))
}

/// `true` when the file carries a requirement-level `## RENAMED ...`
/// section heading (the R7 combination guard).
fn has_renamed_section(spec_md: &str) -> bool {
    let normalized = normalize_line_endings(spec_md);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mask = build_code_fence_mask(&lines);
    lines
        .iter()
        .zip(&mask)
        .any(|(line, fenced)| !fenced && renamed_section_re().is_match(line))
}

/// Verdict of the guard matrix for one directive-bearing requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardOutcome {
    /// At least one op is fresh: the canon block must be pre-synced.
    Apply,
    /// Every op is already reflected in the canon block: no-op.
    AlreadyApplied,
}

/// Run the guard matrix over one directive-bearing requirement and collect
/// ALL violations (rule-tagged) instead of stopping at the first.
///
/// Rule map (design.md section 3): R1 rename source presence + both-present
/// ambiguity, R2 rename target sanity (skipped for already-applied ops),
/// R3 drop presence + justification, R4 multiplicity deadlock, R5 adjusted
/// superset, R6 stale carry, R10 invariants + duplicate rename targets.
pub fn check_guard(
    canon_block_raw: &str,
    delta_block_raw: &str,
    directive: &DirectiveComment,
    requirement_name: &str,
) -> Result<GuardOutcome, Vec<ArchiveError>> {
    let canon_names = scenario_names(canon_block_raw);
    let delta_names = scenario_names(delta_block_raw);
    let canon_count = |name: &str| {
        canon_names
            .iter()
            .filter(|candidate| candidate.as_str() == name)
            .count()
    };
    let delta_count = |name: &str| {
        delta_names
            .iter()
            .filter(|candidate| candidate.as_str() == name)
            .count()
    };

    let mut errors: Vec<ArchiveError> = Vec::new();
    // R5: canon scenarios consumed by directive ops, applied or not.
    let mut consumed: Vec<&str> = Vec::new();
    // R10: targets of fresh renames, for the same-target check.
    let mut fresh_targets: Vec<&str> = Vec::new();
    let mut any_fresh = false;

    for op in &directive.ops {
        match op {
            ScenarioOp::Renamed { from, to } => {
                consumed.push(from);
                if canon_count(from) == 0 && canon_count(to) > 0 {
                    // R1 already-applied branch: R2/R6 short-circuit so an
                    // idempotent re-run stays a no-op.
                    continue;
                }
                any_fresh = true;
                if from == to {
                    errors.push(
                        ArchiveError::new(
                            "R2",
                            format!("rename source and target are identical ({from:?})"),
                        )
                        .with_scenario(from.clone()),
                    );
                    continue;
                }
                let from_count = canon_count(from);
                if from_count == 0 {
                    errors.push(
                        ArchiveError::new(
                            "R1",
                            "rename source not found in the canonical block (count 0); \
                             not an already-applied rename either",
                        )
                        .with_scenario(from.clone()),
                    );
                    continue;
                }
                if from_count > 1 {
                    errors.push(
                        ArchiveError::new(
                            "R4",
                            format!(
                                "rename source occurs {from_count} times in the \
                                 canonical block; renaming one of many is unarchivable"
                            ),
                        )
                        .with_scenario(from.clone()),
                    );
                    continue;
                }
                if canon_count(to) > 0 {
                    errors.push(
                        ArchiveError::new(
                            "R1",
                            "rename source and target both exist in the canonical \
                             block; the intent is ambiguous and will not be guessed"
                                .to_owned(),
                        )
                        .with_scenario(to.clone()),
                    );
                    continue;
                }
                if delta_count(to) == 0 {
                    errors.push(
                        ArchiveError::new("R2", "rename target is not present in the delta block")
                            .with_scenario(to.clone()),
                    );
                }
                if delta_count(from) > 0 {
                    errors.push(
                        ArchiveError::new(
                            "R6",
                            "renamed-from scenario is still listed in the delta \
                             block; carry it under the new name only",
                        )
                        .with_scenario(from.clone()),
                    );
                }
                fresh_targets.push(to);
            }
            ScenarioOp::Dropped {
                name,
                justification,
            } => {
                consumed.push(name);
                let name_count = canon_count(name);
                if name_count == 0 {
                    // R3 already-applied branch: the drop already happened.
                    continue;
                }
                any_fresh = true;
                if justification.trim().is_empty() {
                    errors.push(
                        ArchiveError::new("R3", "drop requires a non-empty justification")
                            .with_scenario(name.clone()),
                    );
                }
                if name_count > 1 {
                    errors.push(
                        ArchiveError::new(
                            "R4",
                            format!(
                                "dropped scenario occurs {name_count} times in the \
                                 canonical block; dropping one of many is unarchivable"
                            ),
                        )
                        .with_scenario(name.clone()),
                    );
                }
                if delta_count(name) > 0 {
                    errors.push(
                        ArchiveError::new(
                            "R6",
                            "dropped scenario is still listed in the delta block; \
                             remove it from the delta",
                        )
                        .with_scenario(name.clone()),
                    );
                }
            }
        }
    }

    // R10: two fresh renames may not compete for the same target name.
    let mut seen_targets: Vec<&str> = Vec::new();
    for target in &fresh_targets {
        if seen_targets.contains(target) {
            errors.push(
                ArchiveError::new(
                    "R10",
                    "two renames target the same scenario name".to_owned(),
                )
                .with_scenario((*target).to_owned()),
            );
        } else {
            seen_targets.push(target);
        }
    }

    // R5 adjusted superset, multiplicity-aware: every canon scenario not
    // consumed by a directive op (from/dropped names, applied or not) must
    // appear in the delta block with at least the same multiplicity.
    let mut canon_remaining: HashMap<&str, usize> = HashMap::new();
    for name in &canon_names {
        *canon_remaining.entry(name.as_str()).or_insert(0) += 1;
    }
    for name in &consumed {
        canon_remaining.remove(name);
    }
    let mut delta_available: HashMap<&str, usize> = HashMap::new();
    for name in &delta_names {
        *delta_available.entry(name.as_str()).or_insert(0) += 1;
    }
    let mut missing: Vec<(&str, usize)> = canon_remaining
        .iter()
        .filter_map(|(name, needed)| {
            let have = delta_available.get(name).copied().unwrap_or(0);
            (have < *needed).then_some((*name, needed - have))
        })
        .collect();
    if !missing.is_empty() {
        missing.sort_unstable();
        let detail = missing
            .iter()
            .map(|(name, count)| {
                if *count > 1 {
                    format!("{name:?} x{count}")
                } else {
                    (*name).to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        errors.push(ArchiveError::new(
            "R5",
            format!("canonical scenario(s) absent from the delta block: {detail}"),
        ));
    }

    if errors.is_empty() {
        Ok(if any_fresh {
            GuardOutcome::Apply
        } else {
            GuardOutcome::AlreadyApplied
        })
    } else {
        for error in &mut errors {
            error
                .requirement
                .get_or_insert_with(|| requirement_name.to_owned());
        }
        Err(errors)
    }
}

/// Replace one named requirement block in the canonical spec's
/// `## Requirements` section with `replacement_raw` (VERBATIM, directive
/// comment included), recomposing the section the way the upstream engine
/// writes it: preamble preserved, blocks joined with blank lines, blank
/// runs of 3+ newlines collapsed to one blank line. Untouched blocks stay
/// byte-stable. An unknown requirement name (or a missing section) is an
/// `R9` error.
pub fn splice_block(
    canon_md: &str,
    requirement_name: &str,
    replacement_raw: &str,
) -> Result<String, ArchiveError> {
    let normalized = normalize_line_endings(canon_md);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mask = build_code_fence_mask(&lines);
    let Some((section_start, section_end)) =
        find_section_bounds(&lines, &mask, requirements_section_re())
    else {
        return Err(ArchiveError::new(
            "R9",
            "canonical spec has no `## Requirements` section",
        ));
    };
    let ranges = block_ranges(&lines, &mask, section_start, section_end);
    if !ranges.iter().any(|(name, _, _)| name == requirement_name) {
        return Err(ArchiveError::new(
            "R9",
            format!("requirement {requirement_name:?} not found in the canonical spec"),
        )
        .with_requirement(requirement_name));
    }

    // Rebuild the section: everything through the heading, the preamble
    // verbatim, then the blocks with exactly one blank line between them.
    let mut out: Vec<String> = lines[..section_start + 1]
        .iter()
        .map(|line| (*line).to_owned())
        .collect();
    out.extend(
        lines[section_start + 1..ranges[0].1]
            .iter()
            .map(|line| (*line).to_owned()),
    );
    for (index, (name, start, end)) in ranges.iter().enumerate() {
        if index > 0 {
            out.push(String::new());
        }
        if name == requirement_name {
            let replacement = normalize_line_endings(replacement_raw)
                .trim_end()
                .to_owned();
            out.extend(replacement.split('\n').map(str::to_owned));
        } else {
            let mut trimmed_end = *end;
            while trimmed_end > *start && lines[trimmed_end - 1].trim().is_empty() {
                trimmed_end -= 1;
            }
            out.extend(lines[*start..trimmed_end].iter().map(|l| (*l).to_owned()));
        }
    }
    let mut section_text = out.join("\n");
    if section_end < lines.len() {
        // Blank line separating the section from the following `## ` heading.
        section_text.push_str("\n\n");
        section_text.push_str(&lines[section_end..].join("\n"));
    } else {
        section_text.push('\n');
    }
    Ok(collapse_blank_runs(&section_text))
}

/// Collapse runs of 3+ newlines to a blank line, porting the upstream
/// normalization applied to rebuilt requirement sections.
fn collapse_blank_runs(text: &str) -> String {
    blank_run_re().replace_all(text, "\n\n").into_owned()
}

/// Discover the change's delta spec files: a recursive walk of
/// `<change_dir>/specs/**/spec.md` mirroring upstream `discoverSpecFiles`
/// (dot-directories pruned, a root-level `specs/spec.md` ignored) with a
/// deterministic sort by capability id (`a`, `a/b`, ...).
pub fn discover_delta_specs(change_dir: &Path) -> Vec<(String, PathBuf)> {
    let specs_root = change_dir.join("specs");
    if !specs_root.is_dir() {
        return Vec::new();
    }
    let mut found: Vec<(String, PathBuf)> = WalkDir::new(&specs_root)
        .min_depth(1)
        .into_iter()
        .filter_entry(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if entry.depth() < 2 || entry.file_name() != "spec.md" || !entry.file_type().is_file() {
                return None;
            }
            let parent = entry.path().parent()?;
            let relative = parent.strip_prefix(&specs_root).ok()?;
            let id = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            Some((id, entry.into_path()))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// The write-free plan the wrapper carries out after the validate gate has
/// passed (design.md section 3, steps 4-7). Factored out of `run` so unit
/// tests never spawn the upstream binary.
#[derive(Debug)]
enum PlanActions {
    /// `--check`: guard verdicts only — zero writes, zero execs.
    Check { verdicts: Vec<GuardVerdict> },
    /// No directive anywhere: plain `openspec archive` passthrough; canon
    /// files are never opened for write.
    Passthrough,
    /// Canon pre-sync writes, then the upstream archive exec.
    Presync { writes: Vec<CanonWrite> },
}

/// Per-requirement guard result surfaced by `--check`.
#[derive(Debug)]
struct GuardVerdict {
    capability: String,
    requirement: String,
    outcome: GuardOutcome,
}

/// One canonical spec file the wrapper pre-syncs before archiving. All
/// directive-bearing requirements targeting the same file are grouped into
/// a single write carrying the final spliced content.
#[derive(Debug)]
struct CanonWrite {
    path: PathBuf,
    content: String,
    capability: String,
    /// `(requirement, op line)` pairs, in plan order, for the success
    /// summary.
    summary: Vec<(String, String)>,
}

/// A directive-bearing delta MODIFIED requirement awaiting canon resolution.
#[derive(Debug)]
struct PendingRequirement {
    capability: String,
    requirement: String,
    /// Delta MODIFIED block raw — the verbatim splice replacement.
    delta_raw: String,
    directive: DirectiveComment,
    canon_path: PathBuf,
}

/// Steps 4-6 of the wrapper flow: discover delta specs, parse directives,
/// reject structural violations (R7/R8), resolve canon and run the guard
/// matrix (R1-R10 + R9 resolution errors), and derive the write plan.
/// Pure against the filesystem except for reads; unit-testable without the
/// upstream binary.
fn plan_actions(
    workspace_root: &Path,
    change: &str,
    check: bool,
) -> Result<PlanActions, Vec<ArchiveError>> {
    let openspec_root = workspace_root.join("openspec");
    let change_dir = openspec_root.join("changes").join(change);

    let mut errors: Vec<ArchiveError> = Vec::new();
    let mut pending: Vec<PendingRequirement> = Vec::new();
    for (capability, delta_path) in discover_delta_specs(&change_dir) {
        let spec_md = match std::fs::read_to_string(&delta_path) {
            Ok(spec_md) => spec_md,
            Err(error) => {
                errors.push(ArchiveError::new(
                    "R9",
                    format!("cannot read delta spec {}: {error}", delta_path.display()),
                ));
                continue;
            }
        };
        let modified = requirement_blocks_with_ranges(&spec_md, modified_requirements_re());
        let ranges: Vec<(usize, usize)> = modified
            .iter()
            .map(|(_, start, end)| (*start, *end))
            .collect();
        if has_renamed_section(&spec_md)
            && modified
                .iter()
                .any(|(block, _, _)| has_unfenced_directive_open(&block.raw))
        {
            errors.push(ArchiveError::new(
                "R7",
                "delta file combines a requirement-level RENAMED section with \
                 scenario-ops directives; v1 does not reason about \
                 rename-then-modify keying",
            ));
        }
        if !directive_lines_outside_blocks(&spec_md, &ranges).is_empty() {
            errors.push(ArchiveError::new(
                "R8",
                "`openspec-scenario-ops` comment found outside any requirement \
                 block; directives live inside MODIFIED requirement blocks only",
            ));
        }
        for (block, _, _) in modified {
            let block_lines: Vec<String> = block.raw.split('\n').map(str::to_owned).collect();
            match parse_directive_comment(&block_lines) {
                Ok(None) => {}
                Ok(Some(directive)) => pending.push(PendingRequirement {
                    canon_path: openspec_root
                        .join("specs")
                        .join(&capability)
                        .join("spec.md"),
                    capability: capability.clone(),
                    requirement: block.name,
                    delta_raw: block.raw,
                    directive,
                }),
                Err(error) => errors.push(error),
            }
        }
    }
    if pending.is_empty() {
        return if errors.is_empty() {
            Ok(PlanActions::Passthrough)
        } else {
            Err(errors)
        };
    }

    // Step 5: resolve canon and run the guard matrix, collecting every
    // violation before anything is written.
    let mut resolved: Vec<(PendingRequirement, String, GuardOutcome)> = Vec::new();
    for req in pending {
        let canon_md = match std::fs::read_to_string(&req.canon_path) {
            Ok(canon_md) => canon_md,
            Err(error) => {
                errors.push(
                    ArchiveError::new(
                        "R9",
                        format!(
                            "canonical spec {} not readable ({error}); refusing to \
                             pre-sync or exec the upstream archive",
                            req.canon_path.display()
                        ),
                    )
                    .with_requirement(req.requirement.clone()),
                );
                continue;
            }
        };
        let canon_block = match split_requirement_blocks(&canon_md)
            .into_iter()
            .find(|block| block.name == req.requirement)
        {
            Some(canon_block) => canon_block,
            None => {
                errors.push(
                    ArchiveError::new(
                        "R9",
                        format!(
                            "MODIFIED requirement not found in the canonical spec {}",
                            req.canon_path.display()
                        ),
                    )
                    .with_requirement(req.requirement.clone()),
                );
                continue;
            }
        };
        match check_guard(
            &canon_block.raw,
            &req.delta_raw,
            &req.directive,
            &req.requirement,
        ) {
            Ok(outcome) => resolved.push((req, canon_md, outcome)),
            Err(mut violations) => errors.append(&mut violations),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    if check {
        return Ok(PlanActions::Check {
            verdicts: resolved
                .into_iter()
                .map(|(req, _, outcome)| GuardVerdict {
                    capability: req.capability,
                    requirement: req.requirement,
                    outcome,
                })
                .collect(),
        });
    }

    // Step 6: splice every fresh (Apply) requirement into its canon file.
    // AlreadyApplied requirements skip the write entirely — no double-write
    // on an idempotent re-run. Requirements sharing one canon file splice
    // sequentially onto the previous result and group into ONE write per
    // path, holding the final content.
    let mut errors: Vec<ArchiveError> = Vec::new();
    let mut writes: Vec<CanonWrite> = Vec::new();
    let mut write_index: HashMap<PathBuf, usize> = HashMap::new();
    let mut spliced: HashMap<PathBuf, String> = HashMap::new();
    for (req, canon_md, outcome) in resolved {
        if outcome != GuardOutcome::Apply {
            continue;
        }
        let base = spliced.get(&req.canon_path).unwrap_or(&canon_md);
        let content = match splice_block(base, &req.requirement, &req.delta_raw) {
            Ok(content) => content,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let summary = req
            .directive
            .ops
            .iter()
            .map(|op| match op {
                ScenarioOp::Renamed { from, to } => format!("renamed {from:?} -> {to:?}"),
                ScenarioOp::Dropped {
                    name,
                    justification,
                } => format!("dropped {name:?} ({justification})"),
            })
            .collect::<Vec<_>>();
        spliced.insert(req.canon_path.clone(), content.clone());
        match write_index.get(&req.canon_path) {
            Some(&index) => {
                let grouped = &mut writes[index];
                grouped.content = content;
                for line in summary {
                    grouped.summary.push((req.requirement.clone(), line));
                }
            }
            None => {
                write_index.insert(req.canon_path.clone(), writes.len());
                writes.push(CanonWrite {
                    path: req.canon_path,
                    content,
                    capability: req.capability,
                    summary: summary
                        .into_iter()
                        .map(|line| (req.requirement.clone(), line))
                        .collect(),
                });
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(PlanActions::Presync { writes })
}

/// `cargo xtask archive <change> [--check]` entry point (design.md section
/// 3). Returns the process exit code: 0 on success (including `--check`
/// verdicts), the upstream exit code on engine failure, 1 on any wrapper
/// reject. Prints like the other xtask commands: plain lines, no color.
pub fn run(workspace_root: &Path, change: &str, check: bool) -> i32 {
    // Step 2: the validate gate — any failure aborts before a single byte
    // is written; child stdout (JSON diagnostics) passes through.
    let mut validate = validate_command(change);
    validate.current_dir(workspace_root);
    match validate.status() {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!("archive: `openspec validate {change}` failed; aborting before any write");
            return status.code().unwrap_or(1);
        }
        Err(error) => {
            eprintln!("archive: failed to exec `openspec validate`: {error}");
            return 1;
        }
    }

    let plan = match plan_actions(workspace_root, change, check) {
        Ok(plan) => plan,
        Err(errors) => {
            eprintln!("archive: guard rejected `{change}`; no files were written:");
            for error in &errors {
                eprintln!("  {error}");
            }
            return 1;
        }
    };

    carry_out_plan(workspace_root, change, check, plan, exec_archive)
}

/// The write/exec arm of the wrapper flow, factored out of `run` so unit
/// tests can prove that a `--check` plan never spawns the upstream binary
/// (the exec is injected). `check` must be the same flag `plan` was built
/// with. Prints like the other xtask commands: plain lines, no color.
fn carry_out_plan(
    workspace_root: &Path,
    change: &str,
    check: bool,
    plan: PlanActions,
    exec: impl FnOnce(&Path, &str) -> i32,
) -> i32 {
    match plan {
        // A directive-free change has nothing to check: report the verdict
        // and stop BEFORE the passthrough exec — `--check` never writes nor
        // spawns.
        PlanActions::Passthrough if check => {
            println!(
                "archive --check {change}: no scenario-ops directives; \
                 passthrough would exec the upstream archive unchanged"
            );
            0
        }
        PlanActions::Passthrough => exec(workspace_root, change),
        PlanActions::Check { verdicts } => {
            println!(
                "archive --check {change}: {} directive-bearing requirement(s)",
                verdicts.len()
            );
            for verdict in &verdicts {
                println!(
                    "  {} :: {} :: {:?}",
                    verdict.capability, verdict.requirement, verdict.outcome
                );
            }
            0
        }
        PlanActions::Presync { writes } => {
            for write in &writes {
                if let Err(error) = std::fs::write(&write.path, &write.content) {
                    eprintln!("archive: failed to write {}: {error}", write.path.display());
                    eprintln!(
                        "archive: canonical pre-sync FAILED midway — earlier \
                         writes may already be applied."
                    );
                    eprintln!(
                        "archive: recovery is idempotent — fix the delta and re-run \
                         `cargo xtask archive {change}`."
                    );
                    return 1;
                }
            }
            println!(
                "archive: pre-synced {} canonical spec file(s)",
                writes.len()
            );
            let code = exec(workspace_root, change);
            if code != 0 {
                eprintln!(
                    "archive: `openspec archive` FAILED AFTER the canonical specs were pre-synced."
                );
                eprintln!(
                    "archive: recovery is idempotent — fix the delta and re-run \
                     `cargo xtask archive {change}`."
                );
            } else {
                for write in &writes {
                    for (requirement, line) in &write.summary {
                        println!("  {} :: {} :: {line}", write.capability, requirement);
                    }
                }
            }
            code
        }
    }
}

/// Step 7: exec `openspec archive <change> --json --yes` and propagate the
/// exit code; stdout/stderr pass through to the caller.
fn exec_archive(workspace_root: &Path, change: &str) -> i32 {
    let mut command = archive_command(change);
    command.current_dir(workspace_root);
    match command.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("archive: failed to exec `openspec archive`: {error}");
            1
        }
    }
}

/// The upstream archive invocation, kept a pure constructor so tests can
/// assert the argument contract without spawning a process.
fn archive_command(change: &str) -> Command {
    let mut command = Command::new("openspec");
    command.args(["archive", change, "--json", "--yes"]);
    command
}

/// The validate-gate invocation (step 2 of the wrapper flow).
fn validate_command(change: &str) -> Command {
    let mut command = Command::new("openspec");
    command.args(["validate", change, "--json"]);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_owned).collect()
    }

    fn parse_ok(text: &str) -> DirectiveComment {
        match parse_directive_comment(&lines(text)) {
            Ok(Some(directive)) => directive,
            other => panic!("expected Ok(Some(directive)) for {text:?}, got {other:?}"),
        }
    }

    fn parse_err(text: &str) -> ArchiveError {
        match parse_directive_comment(&lines(text)) {
            Err(error) => error,
            other => panic!("expected Err for {text:?}, got {other:?}"),
        }
    }

    fn guard_ok(canon: &str, delta: &str, directive: &DirectiveComment) -> GuardOutcome {
        match check_guard(canon, delta, directive, "Feature") {
            Ok(outcome) => outcome,
            Err(errors) => panic!("expected guard to pass, got {errors:?}"),
        }
    }

    fn guard_errs(canon: &str, delta: &str, directive: &DirectiveComment) -> Vec<ArchiveError> {
        match check_guard(canon, delta, directive, "Feature") {
            Ok(outcome) => panic!("expected guard to reject, got {outcome:?}"),
            Err(errors) => errors,
        }
    }

    fn renamed(from: &str, to: &str) -> ScenarioOp {
        ScenarioOp::Renamed {
            from: from.to_owned(),
            to: to.to_owned(),
        }
    }

    fn dropped(name: &str, justification: &str) -> ScenarioOp {
        ScenarioOp::Dropped {
            name: name.to_owned(),
            justification: justification.to_owned(),
        }
    }

    fn directive(ops: Vec<ScenarioOp>) -> DirectiveComment {
        DirectiveComment { ops }
    }

    fn assert_rule(errors: &[ArchiveError], rule: &str) {
        assert!(
            errors.iter().any(|error| error.rule == rule),
            "expected a {rule} violation in {errors:?}"
        );
    }

    // ---- Task 1: directive parser ----

    #[test]
    fn parse_renamed_and_dropped() {
        let parsed = parse_ok(
            "<!-- openspec-scenario-ops\n\
             renamed: Old header -> New header\n\
             dropped: Gone | obsolete per ADR-42\n\
             -->",
        );
        assert_eq!(
            parsed.ops,
            vec![
                renamed("Old header", "New header"),
                dropped("Gone", "obsolete per ADR-42"),
            ]
        );
    }

    #[test]
    fn parse_rejects_unknown_directive_line() {
        let error = parse_err("<!-- openspec-scenario-ops\nfoo: bar\n-->");
        assert_eq!(error.rule, "R8");
        assert!(error.problem.contains("foo"), "error: {error}");
    }

    #[test]
    fn parse_rejects_drop_without_justification() {
        let error = parse_err("<!-- openspec-scenario-ops\ndropped: Gone |\n-->");
        assert_eq!(error.rule, "R3");
        let error = parse_err("<!-- openspec-scenario-ops\ndropped: Gone\n-->");
        assert_eq!(error.rule, "R3");
    }

    #[test]
    fn parse_rejects_delimiter_collision() {
        let error = parse_err("<!-- openspec-scenario-ops\nrenamed: a -> b -> c\n-->");
        assert_eq!(error.rule, "R10");
        let error = parse_err("<!-- openspec-scenario-ops\nrenamed: a|b -> c\n-->");
        assert_eq!(error.rule, "R10");
    }

    #[test]
    fn parse_rejects_scenario_header_in_directive() {
        let error = parse_err("<!-- openspec-scenario-ops\nrenamed: #### Scenario: X -> Y\n-->");
        assert_eq!(error.rule, "R10");
        let error = parse_err("<!-- openspec-scenario-ops\ndropped: Gone | because we MUST\n-->");
        assert_eq!(error.rule, "R10");
    }

    #[test]
    fn parse_none_when_no_comment() {
        let plain = lines("### Requirement: Feature\nBody text.\n#### Scenario: A\n- WHEN");
        assert_eq!(parse_directive_comment(&plain).ok(), Some(None));
    }

    #[test]
    fn parse_rejects_second_comment() {
        let error = parse_err(
            "<!-- openspec-scenario-ops\n\
             renamed: A -> B\n\
             -->\n\
             <!-- openspec-scenario-ops\n\
             dropped: C | why\n\
             -->",
        );
        assert_eq!(error.rule, "R8");
    }

    #[test]
    fn parse_rejects_unterminated_comment() {
        let error =
            parse_err("### Requirement: Feature\n<!-- openspec-scenario-ops\nrenamed: A -> B\n");
        assert_eq!(error.rule, "R8");
    }

    // ---- Task 2: guard matrix ----

    #[test]
    fn guard_accepts_rename_and_drop() {
        let ops = directive(vec![renamed("A", "A2"), dropped("B", "obsolete")]);
        let outcome = guard_ok(
            "### Requirement: Feature\n#### Scenario: A\n#### Scenario: B\n#### Scenario: C",
            "### Requirement: Feature\n#### Scenario: A2\n#### Scenario: C",
            &ops,
        );
        assert_eq!(outcome, GuardOutcome::Apply);
    }

    #[test]
    fn guard_rejects_missing_from() {
        let ops = directive(vec![renamed("Z", "A2")]);
        let errors = guard_errs(
            "#### Scenario: A\n#### Scenario: B",
            "#### Scenario: A2\n#### Scenario: B",
            &ops,
        );
        assert_rule(&errors, "R1");
        let r1 = errors.iter().find(|error| error.rule == "R1").unwrap(); // allow-unwrap
        assert_eq!(r1.scenario.as_deref(), Some("Z"));
        assert!(
            r1.requirement.is_some(),
            "error must name the requirement: {r1}"
        );
    }

    #[test]
    fn guard_rejects_to_in_canon() {
        // `to` exists in canon: the rename must be rejected, never skipped.
        let ops = directive(vec![renamed("A", "A2")]);
        let errors = guard_errs(
            "#### Scenario: A\n#### Scenario: A2",
            "#### Scenario: A2",
            &ops,
        );
        assert!(
            errors
                .iter()
                .any(|error| error.scenario.as_deref() == Some("A2"))
        );
    }

    #[test]
    fn guard_rejects_missing_justification() {
        let ops = directive(vec![dropped("B", "   ")]);
        let errors = guard_errs(
            "#### Scenario: A\n#### Scenario: B",
            "#### Scenario: A",
            &ops,
        );
        assert_rule(&errors, "R3");
    }

    #[test]
    fn guard_rejects_duplicate_target() {
        // canon carries A twice: renaming one of many is a deadlock (R4).
        let ops = directive(vec![renamed("A", "A2")]);
        let errors = guard_errs(
            "#### Scenario: A\n#### Scenario: A\n#### Scenario: B",
            "#### Scenario: A2\n#### Scenario: B",
            &ops,
        );
        assert_rule(&errors, "R4");
    }

    #[test]
    fn guard_rejects_missing_carry() {
        // C stays in canon but the delta no longer lists it (R5).
        let ops = directive(vec![renamed("A", "A2")]);
        let errors = guard_errs(
            "#### Scenario: A\n#### Scenario: B\n#### Scenario: C",
            "#### Scenario: A2\n#### Scenario: B",
            &ops,
        );
        assert_rule(&errors, "R5");
        let r5 = errors.iter().find(|error| error.rule == "R5").unwrap(); // allow-unwrap
        assert!(r5.problem.contains('C'), "error: {r5}");
    }

    #[test]
    fn guard_rejects_stale_carry() {
        // The delta still lists A under its old name (R6).
        let ops = directive(vec![renamed("A", "A2")]);
        let errors = guard_errs(
            "#### Scenario: A\n#### Scenario: B",
            "#### Scenario: A2\n#### Scenario: A\n#### Scenario: B",
            &ops,
        );
        assert_rule(&errors, "R6");
    }

    #[test]
    fn guard_rejects_rename_eq() {
        let ops = directive(vec![renamed("A", "A")]);
        let errors = guard_errs(
            "#### Scenario: A\n#### Scenario: B",
            "#### Scenario: A\n#### Scenario: B",
            &ops,
        );
        assert_rule(&errors, "R2");
    }

    #[test]
    fn guard_rejects_two_renames_same_target() {
        let ops = directive(vec![renamed("A", "X"), renamed("B", "X")]);
        let errors = guard_errs(
            "#### Scenario: A\n#### Scenario: B\n#### Scenario: C",
            "#### Scenario: X\n#### Scenario: C",
            &ops,
        );
        assert_rule(&errors, "R10");
    }

    #[test]
    fn guard_rejects_from_and_to_both_in_canon() {
        let ops = directive(vec![renamed("A", "A2")]);
        let errors = guard_errs(
            "#### Scenario: A\n#### Scenario: A2",
            "#### Scenario: A2",
            &ops,
        );
        assert_rule(&errors, "R1");
        let r1 = errors.iter().find(|error| error.rule == "R1").unwrap(); // allow-unwrap
        assert!(r1.problem.contains("ambiguous"), "error: {r1}");
    }

    #[test]
    fn guard_already_applied() {
        // from absent, to present in canon: already applied, R2 not consulted.
        let ops = directive(vec![renamed("A", "A2")]);
        let outcome = guard_ok("#### Scenario: A2", "#### Scenario: A2", &ops);
        assert_eq!(outcome, GuardOutcome::AlreadyApplied);

        // Mixed: the applied rename no-ops, the fresh drop still applies.
        let mixed = directive(vec![renamed("A", "A2"), dropped("B", "obsolete")]);
        let outcome = guard_ok(
            "#### Scenario: A2\n#### Scenario: B",
            "#### Scenario: A2",
            &mixed,
        );
        assert_eq!(outcome, GuardOutcome::Apply);
    }

    // ---- Task 3a: splice + discovery ----

    #[test]
    fn splice_replaces_block_verbatim() {
        let canon = "# Spec\n\n## Requirements\n\n### Requirement: Alpha\nBody A.\n\n#### Scenario: A1\n- WHEN a\n\n### Requirement: Beta\nBody B.\n\n#### Scenario: B1\n- WHEN b\n\n## Notes\n\nSee docs.\n";
        let replacement = "### Requirement: Alpha\nBody A2.\n\n#### Scenario: A1\n- WHEN a\n\n#### Scenario: A2\n- WHEN a2";
        let spliced = splice_block(canon, "Alpha", replacement).expect("splice ok"); // allow-unwrap
        assert!(
            spliced.contains(replacement),
            "replacement raw must land verbatim: {spliced:?}"
        );
        assert!(
            spliced.contains("### Requirement: Beta\nBody B.\n\n#### Scenario: B1\n- WHEN b"),
            "untouched sibling block must stay byte-stable: {spliced:?}"
        );
        assert!(
            spliced.starts_with("# Spec\n\n## Requirements\n\n"),
            "preamble must be preserved: {spliced:?}"
        );
        assert!(
            spliced.ends_with("## Notes\n\nSee docs.\n"),
            "section tail must be preserved: {spliced:?}"
        );
        assert!(!spliced.contains("\n\n\n"), "no blank runs: {spliced:?}");
        assert!(
            splice_block(canon, "Gamma", replacement).is_err(),
            "unknown requirement name must error"
        );
    }

    #[test]
    fn splice_normalizes_blank_runs() {
        let canon = "## Requirements\n\n\n\n### Requirement: A\nBody.\n\n#### Scenario: X\n\n\n\n### Requirement: B\nBody B.\n";
        let replacement = "### Requirement: A\nBody2.\n\n#### Scenario: X";
        let spliced = splice_block(canon, "A", replacement).expect("splice ok"); // allow-unwrap
        assert!(
            !spliced.contains("\n\n\n"),
            "blank runs must collapse: {spliced:?}"
        );
        let before = split_requirement_blocks(canon);
        let after = split_requirement_blocks(&spliced);
        assert_eq!(after.len(), before.len(), "blocks: {after:?}");
        assert_eq!(after[0].raw, replacement, "A block: {:?}", after[0].raw);
        assert_eq!(after[1].name, "B");
        assert_eq!(after[1].raw, before[1].raw, "B block normalization-stable");
    }

    #[test]
    fn discovery_finds_nested_specs() {
        let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
        let change = dir.path();
        std::fs::create_dir_all(change.join("specs/alpha/beta")).expect("mkdir"); // allow-unwrap
        std::fs::create_dir_all(change.join("specs/.hidden")).expect("mkdir"); // allow-unwrap
        std::fs::write(change.join("specs/alpha/beta/spec.md"), "x").expect("write"); // allow-unwrap
        std::fs::write(change.join("specs/alpha/spec.md"), "x").expect("write"); // allow-unwrap
        std::fs::write(change.join("specs/spec.md"), "root ignored").expect("write"); // allow-unwrap
        std::fs::write(change.join("specs/.hidden/spec.md"), "dot skipped").expect("write"); // allow-unwrap
        std::fs::write(change.join("specs/alpha/beta/other.md"), "not a spec").expect("write"); // allow-unwrap

        let found = discover_delta_specs(change);
        let ids: Vec<&str> = found.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "alpha/beta"], "sorted by id");
        assert_eq!(found[0].1, change.join("specs/alpha/spec.md"));
        assert_eq!(found[1].1, change.join("specs/alpha/beta/spec.md"));
    }

    // ---- Task 3b: wrapper flow ----

    /// Canon spec fixture: requirement `Feature` with scenarios A and C.
    const CANON: &str = "## Requirements\n\n### Requirement: Feature\nBody SHALL work.\n\n#### Scenario: A\n- WHEN a\n\n#### Scenario: C\n- WHEN c\n";

    /// Delta fixture body: MODIFIED block renaming A to A2, carrying C.
    const DELTA_RENAME: &str = "## MODIFIED Requirements\n\n### Requirement: Feature\nBody SHALL work.\n\n<!-- openspec-scenario-ops\nrenamed: A -> A2\n-->\n\n#### Scenario: A2\n- WHEN a2\n\n#### Scenario: C\n- WHEN c\n";

    /// Scratch openspec root with one canon spec and one delta spec.
    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
    }

    fn fixture(canon: &str, delta: &str) -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("openspec/specs/http")).expect("mkdir"); // allow-unwrap
        std::fs::create_dir_all(root.join("openspec/changes/demo/specs/http")).expect("mkdir"); // allow-unwrap
        std::fs::write(root.join("openspec/specs/http/spec.md"), canon).expect("write"); // allow-unwrap
        std::fs::write(root.join("openspec/changes/demo/specs/http/spec.md"), delta)
            .expect("write"); // allow-unwrap
        Fixture { _dir: dir, root }
    }

    #[test]
    fn rejects_renamed_section_combo() {
        let f = fixture(
            CANON,
            "## RENAMED Requirements\n\n### Requirement: Feature -> Feature2\n\n## MODIFIED Requirements\n\n### Requirement: Feature\nBody SHALL work.\n\n<!-- openspec-scenario-ops\nrenamed: A -> A2\n-->\n\n#### Scenario: A2\n- WHEN a2\n\n#### Scenario: C\n- WHEN c\n",
        );
        let errors = match plan_actions(&f.root, "demo", false) {
            Err(errors) => errors,
            other => panic!("expected R7 reject, got {other:?}"),
        };
        assert!(
            errors.iter().any(|error| error.rule == "R7"),
            "expected an R7 violation in {errors:?}"
        );
    }

    #[test]
    fn rejects_directive_outside_block() {
        let f = fixture(
            CANON,
            "## MODIFIED Requirements\n\n<!-- openspec-scenario-ops\nrenamed: A -> A2\n-->\n\n### Requirement: Feature\nBody SHALL work.\n\n#### Scenario: A2\n- WHEN a2\n\n#### Scenario: C\n- WHEN c\n",
        );
        let errors = match plan_actions(&f.root, "demo", false) {
            Err(errors) => errors,
            other => panic!("expected R8 reject, got {other:?}"),
        };
        assert!(
            errors.iter().any(|error| error.rule == "R8"),
            "expected an R8 violation in {errors:?}"
        );
    }

    #[test]
    fn rejects_missing_requirement() {
        let f = fixture(
            "## Requirements\n\n### Requirement: Other\nBody.\n\n#### Scenario: O\n- WHEN o\n",
            DELTA_RENAME,
        );
        let errors = match plan_actions(&f.root, "demo", false) {
            Err(errors) => errors,
            other => panic!("expected R9 reject, got {other:?}"),
        };
        let r9 = errors
            .iter()
            .find(|error| error.rule == "R9")
            .expect("R9 error"); // allow-unwrap
        assert_eq!(
            r9.requirement.as_deref(),
            Some("Feature"),
            "error must name the requirement: {r9}"
        );
        assert!(
            r9.problem.contains("not found"),
            "error must state the miss: {r9}"
        );
    }

    #[test]
    fn passthrough_when_no_directives() {
        let f = fixture(
            CANON,
            "## MODIFIED Requirements\n\n### Requirement: Feature\nBody SHALL work.\n\n#### Scenario: A\n- WHEN a\n\n#### Scenario: C\n- WHEN c\n",
        );
        assert!(
            matches!(
                plan_actions(&f.root, "demo", false),
                Ok(PlanActions::Passthrough)
            ),
            "a directive-free delta must plan a passthrough"
        );
        let args: Vec<String> = archive_command("demo")
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["archive", "demo", "--json", "--yes"]);
    }

    #[test]
    fn check_mode_no_writes() {
        let f = fixture(CANON, DELTA_RENAME);
        let canon_path = f.root.join("openspec/specs/http/spec.md");
        let before_bytes = std::fs::read(&canon_path).expect("read canon"); // allow-unwrap
        let before_mtime = std::fs::metadata(&canon_path)
            .and_then(|meta| meta.modified())
            .expect("mtime"); // allow-unwrap

        let verdicts = match plan_actions(&f.root, "demo", true) {
            Ok(PlanActions::Check { verdicts }) => verdicts,
            other => panic!("expected a check plan, got {other:?}"),
        };

        assert_eq!(verdicts.len(), 1, "verdicts: {verdicts:?}");
        assert_eq!(verdicts[0].capability, "http");
        assert_eq!(verdicts[0].requirement, "Feature");
        assert_eq!(verdicts[0].outcome, GuardOutcome::Apply);
        let after_bytes = std::fs::read(&canon_path).expect("read canon"); // allow-unwrap
        assert_eq!(before_bytes, after_bytes, "--check must not write canon");
        let after_mtime = std::fs::metadata(&canon_path)
            .and_then(|meta| meta.modified())
            .expect("mtime"); // allow-unwrap
        assert_eq!(before_mtime, after_mtime, "canon mtime must be unchanged");
    }

    #[test]
    fn check_mode_passthrough_no_exec() {
        let f = fixture(
            CANON,
            "## MODIFIED Requirements\n\n### Requirement: Feature\nBody SHALL work.\n\n#### Scenario: A\n- WHEN a\n\n#### Scenario: C\n- WHEN c\n",
        );
        // Plan seam: `--check` on a directive-free change still plans a
        // passthrough, not a verdict plan.
        assert!(
            matches!(
                plan_actions(&f.root, "demo", true),
                Ok(PlanActions::Passthrough)
            ),
            "a directive-free delta must plan a passthrough even under --check"
        );
        // Run-level seam: the injected exec panics if the passthrough path
        // ever reaches the upstream binary — `--check` must return success
        // without spawning it.
        let code = carry_out_plan(
            &f.root,
            "demo",
            true,
            PlanActions::Passthrough,
            |_root, _change| {
                panic!("--check must not exec the upstream archive on a directive-free change");
            },
        );
        assert_eq!(code, 0);
    }

    #[test]
    fn parse_ignores_fenced_directive_example() {
        // A directive comment quoted inside a fenced code example is body
        // text, not an op (fence-mask parity with the other extractors).
        let block = lines(
            "### Requirement: Feature\nBody text.\n\n```text\n\
             <!-- openspec-scenario-ops\nrenamed: A -> B\n-->\n```\n\n\
             #### Scenario: A\n- WHEN a",
        );
        assert_eq!(
            parse_directive_comment(&block).ok(),
            Some(None),
            "a fenced directive example must not parse as real ops"
        );

        // R7 parity, directive side: a real RENAMED section plus a fenced
        // directive example in the MODIFIED block does not trip the guard.
        let f = fixture(
            CANON,
            "## RENAMED Requirements\n\n### Requirement: Old -> New\n\n## MODIFIED Requirements\n\n### Requirement: Feature\nBody SHALL work.\n\n```text\n<!-- openspec-scenario-ops\nrenamed: A -> A2\n-->\n```\n\n#### Scenario: A\n- WHEN a\n\n#### Scenario: C\n- WHEN c\n",
        );
        assert!(
            matches!(
                plan_actions(&f.root, "demo", false),
                Ok(PlanActions::Passthrough)
            ),
            "a fenced directive example must not trip R7 or parse as ops"
        );

        // R7 parity, section side: a RENAMED section mentioned only inside
        // a fence is not a RENAMED section; the real directive still plans
        // a presync.
        let f = fixture(
            CANON,
            "## MODIFIED Requirements\n\n### Requirement: Feature\nBody SHALL work.\n\n<!-- openspec-scenario-ops\nrenamed: A -> A2\n-->\n\n#### Scenario: A2\n- WHEN a2\n\n#### Scenario: C\n- WHEN c\n\n```text\n## RENAMED Requirements\n```\n",
        );
        assert!(
            matches!(
                plan_actions(&f.root, "demo", false),
                Ok(PlanActions::Presync { .. })
            ),
            "a fenced RENAMED mention must not trip R7; the real directive applies"
        );
    }

    #[test]
    fn presync_groups_writes_by_path() {
        // Two directive-bearing requirements targeting one canon file must
        // splice sequentially into a single write carrying the final
        // content and both summaries.
        let canon = "## Requirements\n\n### Requirement: Feature\nBody SHALL work.\n\n#### Scenario: A\n- WHEN a\n\n#### Scenario: C\n- WHEN c\n\n### Requirement: Extra\nBody SHALL hold.\n\n#### Scenario: D\n- WHEN d\n";
        let delta = "## MODIFIED Requirements\n\n### Requirement: Feature\nBody SHALL work.\n\n<!-- openspec-scenario-ops\nrenamed: A -> A2\n-->\n\n#### Scenario: A2\n- WHEN a2\n\n#### Scenario: C\n- WHEN c\n\n### Requirement: Extra\nBody SHALL hold.\n\n<!-- openspec-scenario-ops\nrenamed: D -> D2\n-->\n\n#### Scenario: D2\n- WHEN d2\n";
        let f = fixture(canon, delta);
        let writes = match plan_actions(&f.root, "demo", false) {
            Ok(PlanActions::Presync { writes }) => writes,
            other => panic!("expected a presync plan, got {other:?}"),
        };
        assert_eq!(writes.len(), 1, "one canon file must yield one write");
        let write = &writes[0];
        assert!(
            write.content.contains("#### Scenario: A2")
                && write.content.contains("#### Scenario: D2"),
            "the single write must carry both splices: {:?}",
            write.content
        );
        assert_eq!(write.summary.len(), 2, "summary keeps both requirements");
        assert_eq!(
            write.summary,
            vec![
                ("Feature".to_owned(), "renamed \"A\" -> \"A2\"".to_owned()),
                ("Extra".to_owned(), "renamed \"D\" -> \"D2\"".to_owned()),
            ]
        );
    }

    #[test]
    fn presync_groups_writes_mixed_outcomes() {
        // One Apply requirement plus one AlreadyApplied requirement sharing
        // a canon file must yield exactly ONE write: the Apply splice lands
        // (directive comment included), the AlreadyApplied block is carried
        // through byte-stable, and the summary lists only the Apply
        // requirement's op line.
        let canon = "## Requirements\n\n### Requirement: Feature\nBody SHALL work.\n\n#### Scenario: A\n- WHEN a\n\n#### Scenario: C\n- WHEN c\n\n### Requirement: Extra\nBody SHALL hold.\n\n#### Scenario: D2\n- WHEN d2\n";
        let delta = "## MODIFIED Requirements\n\n### Requirement: Feature\nBody SHALL work.\n\n<!-- openspec-scenario-ops\nrenamed: A -> A2\n-->\n\n#### Scenario: A2\n- WHEN a2\n\n#### Scenario: C\n- WHEN c\n\n### Requirement: Extra\nBody SHALL hold.\n\n<!-- openspec-scenario-ops\nrenamed: D -> D2\n-->\n\n#### Scenario: D2\n- WHEN d2\n";
        let f = fixture(canon, delta);
        let writes = match plan_actions(&f.root, "demo", false) {
            Ok(PlanActions::Presync { writes }) => writes,
            other => panic!("expected a presync plan, got {other:?}"),
        };
        assert_eq!(writes.len(), 1, "one canon file must yield one write");
        let write = &writes[0];
        assert_eq!(
            write.content.matches("openspec-scenario-ops").count(),
            1,
            "only the Apply splice may carry a directive comment: {:?}",
            write.content
        );
        assert!(
            write
                .content
                .contains("### Requirement: Feature\nBody SHALL work.\n\n<!-- openspec-scenario-ops\nrenamed: A -> A2\n-->\n\n#### Scenario: A2"),
            "the Apply replacement block must land verbatim: {:?}",
            write.content
        );
        assert!(
            write.content.contains(
                "### Requirement: Extra\nBody SHALL hold.\n\n#### Scenario: D2\n- WHEN d2"
            ),
            "the AlreadyApplied block must stay unchanged: {:?}",
            write.content
        );
        assert_eq!(
            write.summary,
            vec![("Feature".to_owned(), "renamed \"A\" -> \"A2\"".to_owned())],
            "summary carries only the Apply requirement's op line"
        );
    }

    #[test]
    fn guard_ignores_fenced_headers() {
        let spec = "\
# Sample Specification

## Requirements

### Requirement: Real
Body text with SHALL apply.

```text
### Requirement: Fake
#### Scenario: FakeScenario
```

#### Scenario: A

## Notes

### Requirement: AfterSection
";
        let blocks = split_requirement_blocks(spec);
        assert_eq!(blocks.len(), 1, "blocks: {blocks:?}");
        assert_eq!(blocks[0].name, "Real");
        assert_eq!(scenario_names(&blocks[0].raw), vec!["A".to_owned()]);

        // The guard sees the same fence-masked view of both blocks.
        let canon = blocks[0].raw.clone();
        let delta = "### Requirement: Real\n#### Scenario: A2";
        let ops = directive(vec![renamed("A", "A2")]);
        assert_eq!(guard_ok(&canon, delta, &ops), GuardOutcome::Apply);
    }
}
