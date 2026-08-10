//! Corpus zero-false-positives gate for `camel lint`.
//!
//! Discovers every in-tree route file (`examples/**/*.{yaml,json}` and
//! `crates/**/tests/fixtures/**/*.{yaml,json}`), runs the production lint
//! catalog over each, and asserts that the set of emitted diagnostics exactly
//! matches the committed baseline (`fixtures/lint-corpus-baseline.ron`).
//!
//! Contract (Task 3.2 of `openspec/changes/add-camel-lint`):
//! - every emitted diagnostic MUST be in the baseline (a diagnostic outside
//!   the baseline is a false positive — the gate names the file + code);
//! - every baseline diagnostic MUST be emitted (a missing one is a regression
//!   — a real defect may have been fixed, or a rule weakened).
//!
//! The baseline contains ONLY agreed real defects and expected by-design
//! notes (e.g. `unverified-scheme` Info for components intentionally skipped
//! by `register_builtin_components_for_lint`). Suspected false positives are
//! reported, never baselined.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use camel_cli::commands::lint::production_engine;

/// (DiagnosticCode Display string, Severity Display string) per file.
type CodeSev = (String, String);
/// file-relative-path -> set of (code, severity).
type EmittedMap = BTreeMap<String, BTreeSet<CodeSev>>;

/// Workspace root, resolved from this crate's manifest dir.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root canonicalizes")
}

/// Glob `pattern` (absolute) and append matches to `out`.
fn collect(pattern: &str, out: &mut BTreeSet<PathBuf>) {
    for entry in glob::glob(pattern).expect("glob pattern compiles") {
        match entry {
            Ok(p) => {
                out.insert(p);
            }
            Err(e) => panic!("glob iteration error on `{pattern}`: {e}"),
        }
    }
}

/// Discover + dedup the corpus, returning (relative_path_string, full_path).
///
/// Globs (per Task 3.2): `examples/**/*.{yaml,json}` and
/// `crates/**/tests/fixtures/**/*.{yaml,json}`, deduplicated. The crates
/// glob is scoped to `tests/fixtures` so non-route files (schema JSON,
/// template routes, example data) stay out of the gate.
///
/// `examples/validator/schemas/` is also excluded: those YAML/JSON files are
/// JSON-Schema payload definitions for the validator component (they carry
/// top-level `type`/`properties`/`required`), not route definitions, so
/// linting them as routes would emit spurious R-SCHEMA errors.
fn discover_corpus() -> Vec<(String, PathBuf)> {
    let root = workspace_root();
    let mut found = BTreeSet::new();
    for ext in ["yaml", "yml", "json"] {
        let pat = root
            .join("examples")
            .join("**")
            .join(format!("*.{ext}"))
            .to_string_lossy()
            .into_owned();
        collect(&pat, &mut found);
        let pat = root
            .join("crates")
            .join("**")
            .join("tests")
            .join("fixtures")
            .join("**")
            .join(format!("*.{ext}"))
            .to_string_lossy()
            .into_owned();
        collect(&pat, &mut found);
    }
    // Exclude non-route payload schemas (see function doc).
    let excluded = root.join("examples").join("validator").join("schemas");
    found
        .into_iter()
        .filter(|p| !p.starts_with(&excluded))
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap_or_else(|_| panic!("corpus path {p:?} not under workspace root {root:?}"))
                .to_string_lossy()
                .into_owned();
            (rel, p)
        })
        .collect()
}

/// Run the engine over the whole corpus, returning the emitted map.
async fn run_corpus() -> EmittedMap {
    let engine = production_engine()
        .await
        .expect("production engine builds for corpus gate");
    let corpus = discover_corpus();
    assert!(
        !corpus.is_empty(),
        "corpus glob found zero files — discovery is broken"
    );

    let mut emitted: EmittedMap = BTreeMap::new();
    for (rel, full) in &corpus {
        let source = std::fs::read_to_string(full)
            .unwrap_or_else(|e| panic!("read corpus file {full:?}: {e}"));
        for diag in engine.lint(&source) {
            emitted
                .entry(rel.clone())
                .or_default()
                .insert((diag.code.to_string(), diag.severity.to_string()));
        }
    }
    emitted
}

/// Parse the baseline RON into an EmittedMap.
fn load_baseline() -> EmittedMap {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lint-corpus-baseline.ron");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read baseline {path:?}: {e}"));
    let list: Vec<(String, Vec<CodeSev>)> =
        ron::from_str(&text).unwrap_or_else(|e| panic!("parse baseline RON: {e}"));
    let mut map: EmittedMap = BTreeMap::new();
    for (file, codes) in list {
        for cs in codes {
            map.entry(file.clone()).or_default().insert(cs);
        }
    }
    map
}

/// Compare emitted against baseline. Returns the list of failures (empty on
/// match). Two failure kinds: false-positive (emitted not in baseline) and
/// missing-regression (baseline not emitted).
fn compare(emitted: &EmittedMap, baseline: &EmittedMap) -> Vec<String> {
    let mut failures = Vec::new();

    // False positives: emitted \ baseline.
    for (file, codes) in emitted {
        let base = baseline.get(file);
        for cs in codes {
            if !base.is_some_and(|b| b.contains(cs)) {
                failures.push(format!(
                    "FALSE-POSITIVE: {file} emits {}({}) not in baseline",
                    cs.0, cs.1
                ));
            }
        }
    }

    // Missing regressions: baseline \ emitted.
    for (file, codes) in baseline {
        let got = emitted.get(file);
        for cs in codes {
            if !got.is_some_and(|g| g.contains(cs)) {
                failures.push(format!(
                    "MISSING-REGRESSION: {file} baseline {}({}) not emitted",
                    cs.0, cs.1
                ));
            }
        }
    }

    failures
}

#[tokio::test]
async fn corpus_zero_false_positives() {
    let emitted = run_corpus().await;
    let baseline = load_baseline();
    let failures = compare(&emitted, &baseline);
    if !failures.is_empty() {
        panic!(
            "corpus gate failed ({} mismatch(es)):\n  - {}\n\n\
             If these are real defects, add them to \
             tests/fixtures/lint-corpus-baseline.ron with a justification.\n\
             If they are suspected false positives, report them — do NOT \
             baseline.",
            failures.len(),
            failures.join("\n  - ")
        );
    }
}

#[tokio::test]
async fn corpus_gate_detects_false_positive() {
    // Self-contained probe: run the real corpus, then inject a sentinel
    // diagnostic that the production catalog never emits, and assert the
    // gate's "emitted ⊄ baseline" branch fires and names the file + code.
    // This does NOT pollute the real gate (no permanent rule/rule injection).
    let mut emitted = run_corpus().await;
    let baseline = load_baseline();

    // Pick any corpus file; inject a sentinel code guaranteed absent from the
    // baseline (the catalog never emits `LINT-GATE-PROBE`).
    let probe_file = emitted
        .keys()
        .next()
        .cloned()
        .expect("corpus non-empty for FP probe");
    emitted
        .entry(probe_file.clone())
        .or_default()
        .insert(("LINT-GATE-PROBE".to_string(), "error".to_string()));

    let failures = compare(&emitted, &baseline);
    assert!(
        !failures.is_empty(),
        "gate must FAIL when an unbasetined diagnostic is emitted"
    );
    let named = failures
        .iter()
        .any(|f| f.contains(&probe_file) && f.contains("LINT-GATE-PROBE"));
    assert!(
        named,
        "gate failure must name the file + code; got:\n  - {}",
        failures.join("\n  - ")
    );
}
