//! Unit tests for the phase-2 dynamic-flag lowering engine
//! (`lower_dynamic_flags`): the raw tail after the document reference
//! resolved against the document's declared `args:` block.

use std::ffi::OsString;
use std::path::Path;

use super::document::{self, JobArgType, JobDocError};
use super::{DynamicFlagError, lower_dynamic_flags, tail_terminator_boundary};

/// A `.job.yaml` path inside a tempdir-free constant (the parser only
/// inspects the suffix).
fn doc_path() -> std::path::PathBuf {
    Path::new("fixtures/job.job.yaml").to_path_buf()
}

/// Real declarations via the help parser, mirroring the production
/// caller (`parse_job_document_for_help(...).args`).
fn declarations_for(text: &str) -> Option<document::JobArgumentDeclarations> {
    document::parse_job_document_for_help(&doc_path(), text)
        .expect("document parses for help")
        .args
}

fn tail(tokens: &[&str]) -> Vec<OsString> {
    tokens.iter().map(OsString::from).collect()
}

const NAME_DOC: &str = r#"
args:
  name:
    required: true
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
"#;

const COUNT_INT_DOC: &str = r#"
args:
  count:
    type: int
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
"#;

const VERBOSE_BOOL_DOC: &str = r#"
args:
  verbose:
    type: bool
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
"#;

const VERBOSE_BOOL_DEFAULT_DOC: &str = r#"
args:
  verbose:
    type: bool
    default: "true"
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
"#;

const USER_NAME_DOC: &str = r#"
args:
  user_name:
    required: true
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
"#;

const DOCUMENT_AND_BOOL_NEGATION_DOC: &str = r#"
args:
  document:
    type: string
  verbose:
    type: bool
  no_verbose:
    type: string
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:transform
routeFiles:
  - routes/job-route.yaml
"#;

#[test]
fn lower_string_flag_to_pair() {
    let decls = declarations_for(NAME_DOC);
    let lowered = lower_dynamic_flags(decls.as_ref(), &tail(&["--name", "world"]), &[])
        .expect("space form lowers");
    assert_eq!(
        lowered.pairs.last(),
        Some(&("name".to_string(), "world".to_string()))
    );
    let equals = lower_dynamic_flags(decls.as_ref(), &tail(&["--name=world"]), &[])
        .expect("equals form lowers");
    assert_eq!(equals.pairs, lowered.pairs, "both forms are byte-identical");
}

#[test]
fn lower_last_wins_within_form() {
    let decls = declarations_for(NAME_DOC);
    let lowered = lower_dynamic_flags(
        decls.as_ref(),
        &tail(&["--name", "first", "--name", "second"]),
        &[],
    )
    .expect("repeated flag lowers");
    assert_eq!(
        lowered.pairs,
        vec![("name".to_string(), "second".to_string())],
        "last occurrence wins"
    );
}

#[test]
fn lower_int_flag_flows_to_coercion() {
    let decls = declarations_for(COUNT_INT_DOC);
    let lowered = lower_dynamic_flags(decls.as_ref(), &tail(&["--count", "notanint"]), &[])
        .expect("flag lowers; coercion is downstream");
    let err = document::parse_job_document_with_args(&doc_path(), COUNT_INT_DOC, &lowered.pairs)
        .unwrap_err();
    match &err {
        JobDocError::ArgumentCoercion {
            name,
            expected,
            raw,
        } => {
            assert_eq!(name, "count");
            assert!(matches!(expected, JobArgType::Int), "got {expected:?}");
            assert_eq!(raw, "notanint");
        }
        other => panic!("expected ArgumentCoercion, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("count") && msg.contains("int") && msg.contains("notanint"),
        "coercion diagnostic names all three: {msg}"
    );
}

#[test]
fn lower_bool_bare_and_negated() {
    let decls = declarations_for(VERBOSE_BOOL_DEFAULT_DOC);
    let bare =
        lower_dynamic_flags(decls.as_ref(), &tail(&["--verbose"]), &[]).expect("bare bool lowers");
    assert_eq!(
        bare.pairs,
        vec![("verbose".to_string(), "true".to_string())]
    );
    let negated = lower_dynamic_flags(decls.as_ref(), &tail(&["--no-verbose"]), &[])
        .expect("negated bool lowers");
    assert_eq!(
        negated.pairs,
        vec![("verbose".to_string(), "false".to_string())]
    );
    let empty = lower_dynamic_flags(decls.as_ref(), &tail(&[]), &[]).expect("empty tail");
    assert!(
        empty.pairs.is_empty(),
        "no verbose pair: the declared default applies downstream"
    );
}

#[test]
fn lower_bool_value_form_rejected_naming_spellings() {
    let decls = declarations_for(VERBOSE_BOOL_DOC);
    for spelling in ["--verbose=false", "--no-verbose=false"] {
        let err = lower_dynamic_flags(decls.as_ref(), &tail(&[spelling]), &[]).unwrap_err();
        match &err {
            DynamicFlagError::BoolFlagValue { name } => assert_eq!(name, "verbose"),
            other => panic!("expected BoolFlagValue for {spelling}, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains("--verbose")
                && msg.contains("--no-verbose")
                && msg.contains("--arg verbose=false"),
            "diagnostic names every usable spelling: {msg}"
        );
    }
}

#[test]
fn lower_stray_positional_after_bool_rejected() {
    let decls = declarations_for(VERBOSE_BOOL_DOC);
    let err = lower_dynamic_flags(decls.as_ref(), &tail(&["--verbose", "false"]), &[]).unwrap_err();
    match &err {
        DynamicFlagError::UnexpectedPositional { value } => assert_eq!(value, "false"),
        other => panic!("expected UnexpectedPositional, got {other:?}"),
    }
}

#[test]
fn lower_contradictory_bool_rejected() {
    let decls = declarations_for(VERBOSE_BOOL_DOC);
    let err = lower_dynamic_flags(decls.as_ref(), &tail(&["--verbose", "--no-verbose"]), &[])
        .unwrap_err();
    match &err {
        DynamicFlagError::ContradictoryBool { name } => assert_eq!(name, "verbose"),
        other => panic!("expected ContradictoryBool, got {other:?}"),
    }
}

#[test]
fn lower_negation_of_non_bool_is_unknown() {
    let decls = declarations_for(COUNT_INT_DOC);
    let err = lower_dynamic_flags(decls.as_ref(), &tail(&["--no-count"]), &[]).unwrap_err();
    let DynamicFlagError::Clap(rendered) = &err else {
        panic!("expected Clap, got {err:?}")
    };
    assert!(
        rendered.contains("--no-count"),
        "names the token: {rendered}"
    );
}

#[test]
fn lower_alias_spellings_rejected() {
    let decls = declarations_for(USER_NAME_DOC);
    for spelling in ["--user-name", "--user", "-u"] {
        let err = lower_dynamic_flags(decls.as_ref(), &tail(&[spelling, "x"]), &[]).unwrap_err();
        let DynamicFlagError::Clap(rendered) = &err else {
            panic!("expected Clap for {spelling}, got {err:?}")
        };
        assert!(
            rendered.contains(spelling),
            "names the alias token {spelling}: {rendered}"
        );
    }
}

#[test]
fn lower_undeclared_flag_renders_clap_error_with_hint() {
    let decls = declarations_for(NAME_DOC);
    let err = lower_dynamic_flags(decls.as_ref(), &tail(&["--nmae", "x"]), &[]).unwrap_err();
    let DynamicFlagError::Clap(rendered) = &err else {
        panic!("expected Clap, got {err:?}")
    };
    assert!(rendered.contains("--nmae"), "names the typo: {rendered}");
    assert!(
        rendered.contains("--name"),
        "suggests the declared flag: {rendered}"
    );
    // clap 4.x byte-pin
    assert_eq!(
        rendered,
        "error: unexpected argument '--nmae' found\n\n  tip: a similar argument exists: '--name'\n  tip: to pass '--nmae' as a value, use '-- --nmae'\n\nUsage: camel job --name <NAME> [FILE] [DYNAMIC]...\n"
    );
}

#[test]
fn lower_empty_tail_is_identity() {
    let decls = declarations_for(NAME_DOC);
    let arg_pairs = vec![
        ("a".to_string(), "1".to_string()),
        ("b".to_string(), "2".to_string()),
    ];
    let lowered = lower_dynamic_flags(decls.as_ref(), &tail(&[]), &arg_pairs)
        .expect("empty tail is identity");
    assert_eq!(lowered.pairs, arg_pairs);
    assert!(!lowered.help);
    assert_eq!(lowered.report, None);
}

#[test]
fn lower_tail_statics_recovered() {
    let decls = declarations_for(NAME_DOC);
    let lowered = lower_dynamic_flags(
        decls.as_ref(),
        &tail(&["--name", "w", "--report", "r.json", "--arg", "a=1"]),
        &[],
    )
    .expect("statics recover alongside a dynamic flag");
    assert_eq!(
        lowered.report.as_deref(),
        Some(Path::new("r.json")),
        "tail --report recovers"
    );
    assert!(
        lowered.pairs.contains(&("a".to_string(), "1".to_string())),
        "tail --arg pair recovers: {:?}",
        lowered.pairs
    );
    assert!(
        lowered
            .pairs
            .contains(&("name".to_string(), "w".to_string())),
        "dynamic pair recovers: {:?}",
        lowered.pairs
    );
    assert!(!lowered.help);
}

#[test]
fn lower_help_precedence_short_circuits() {
    let decls = declarations_for(NAME_DOC);
    let lowered = lower_dynamic_flags(decls.as_ref(), &tail(&["--nmae", "x", "--help"]), &[])
        .expect("help wins over flag errors");
    assert!(lowered.help, "--help short-circuits before validation");
    assert!(lowered.pairs.is_empty());
    assert_eq!(lowered.report, None);
}

#[test]
fn lower_cross_form_conflict_both_orders() {
    let decls = declarations_for(NAME_DOC);
    let flag_first = lower_dynamic_flags(
        decls.as_ref(),
        &tail(&["--name", "a"]),
        &[("name".to_string(), "b".to_string())],
    )
    .unwrap_err();
    let arg_first = lower_dynamic_flags(
        decls.as_ref(),
        &tail(&["--arg", "name=b", "--name", "a"]),
        &[],
    )
    .unwrap_err();
    for err in [&flag_first, &arg_first] {
        match err {
            DynamicFlagError::CrossFormConflict { name } => assert_eq!(name, "name"),
            other => panic!("expected CrossFormConflict, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains("'name'") && msg.contains("--name") && msg.contains("--arg name=VALUE"),
            "diagnostic names both forms: {msg}"
        );
    }
}

#[test]
fn lower_dynamic_on_undeclared_document_errors() {
    let err = lower_dynamic_flags(None, &tail(&["--name", "x"]), &[]).unwrap_err();
    match &err {
        DynamicFlagError::UndeclaredDocument { flag } => assert_eq!(flag, "name"),
        other => panic!("expected UndeclaredDocument, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("args:") && msg.contains("--arg"),
        "diagnostic points at the args: block and --arg: {msg}"
    );

    let statics_only = lower_dynamic_flags(None, &tail(&["--arg", "a=1"]), &[])
        .expect("statics-only tail on a legacy document is not an error");
    assert!(
        statics_only
            .pairs
            .contains(&("a".to_string(), "1".to_string())),
        "statics-only tail recovers: {:?}",
        statics_only.pairs
    );
}

#[test]
fn lower_ids_do_not_collide_with_declared_names() {
    let decls = declarations_for(DOCUMENT_AND_BOOL_NEGATION_DOC);
    let lowered = lower_dynamic_flags(
        decls.as_ref(),
        &tail(&["--document", "x", "--verbose", "--no_verbose", "y"]),
        &[],
    )
    .expect("colon-namespaced IDs keep declared names distinct");
    assert!(
        lowered
            .pairs
            .contains(&("document".to_string(), "x".to_string())),
        "declared document flag does not collide with the positional ID: {:?}",
        lowered.pairs
    );
    assert!(
        lowered
            .pairs
            .contains(&("verbose".to_string(), "true".to_string())),
        "bare bool lowers true: {:?}",
        lowered.pairs
    );
    assert!(
        lowered
            .pairs
            .contains(&("no_verbose".to_string(), "y".to_string())),
        "underscore name is a declared flag, not the dash negation: {:?}",
        lowered.pairs
    );
    assert_eq!(
        lowered.pairs.len(),
        3,
        "exactly the three declared pairs: {:?}",
        lowered.pairs
    );
}

/// The `--` terminator boundary is computable only from RAW argv: clap
/// strips a `--` that precedes tail capture but keeps one inside an
/// already-started tail (shapes probed against the real `JobArgs`
/// clap shape, pinned in the change design). `Some(0)` means the
/// whole tail is post-terminator; `Some(i)` with `i >= 1` means the
/// terminator survives at `tail[i]` — it belongs to neither side of
/// the split.
#[test]
fn tail_boundary_shapes() {
    let os = |tokens: &[&str]| -> Vec<OsString> { tokens.iter().map(OsString::from).collect() };

    // (A) `doc.yaml -- --name x`: the `--` is clap-stripped before
    // capture — the whole tail is post-terminator.
    let dyn_tail = os(&["--name", "x"]);
    let argv = os(&["camel", "job", "doc.yaml", "--", "--name", "x"]);
    assert_eq!(tail_terminator_boundary(&argv, &dyn_tail), Some(0));

    // (B) `doc.yaml --name w -- --literal`: capture started before the
    // `--`, so it SURVIVES at tail index 2; everything before it is
    // reparseable, everything after it is literal (the terminator
    // itself belongs to neither side).
    let dyn_tail = os(&["--name", "w", "--", "--literal"]);
    let argv = os(&["camel", "job", "doc.yaml", "--name", "w", "--", "--literal"]);
    let boundary = tail_terminator_boundary(&argv, &dyn_tail)
        .expect("surviving terminator yields an in-tail boundary");
    assert_eq!(boundary, 2, "the terminator sits at tail[2]");
    assert_eq!(
        &dyn_tail[..boundary],
        &os(&["--name", "w"])[..],
        "pre-terminator tail is reparseable"
    );
    assert_eq!(
        &dyn_tail[boundary + 1..],
        &os(&["--literal"])[..],
        "post-terminator tokens are literals"
    );

    // (C) `-- doc.yaml --name w`: the `--` is consumed before the
    // positional — the whole (stripped) tail is post-terminator.
    let dyn_tail = os(&["--name", "w"]);
    let argv = os(&["camel", "job", "--", "doc.yaml", "--name", "w"]);
    assert_eq!(tail_terminator_boundary(&argv, &dyn_tail), Some(0));

    // (D) `doc.yaml -- plain`: stripped terminator, literal-only tail.
    let dyn_tail = os(&["plainvalue"]);
    let argv = os(&["camel", "job", "doc.yaml", "--", "plainvalue"]);
    assert_eq!(tail_terminator_boundary(&argv, &dyn_tail), Some(0));

    // (E) `doc.yaml --`: empty tail — boundary Some(0) must split into
    // two empty halves so the run proceeds (no literals, no error).
    let dyn_tail: Vec<OsString> = Vec::new();
    let argv = os(&["camel", "job", "doc.yaml", "--"]);
    assert_eq!(tail_terminator_boundary(&argv, &dyn_tail), Some(0));

    // No terminator anywhere: the whole tail is reparseable.
    let dyn_tail = os(&["--name", "x"]);
    let argv = os(&["camel", "job", "doc.yaml", "--name", "x"]);
    assert_eq!(tail_terminator_boundary(&argv, &dyn_tail), None);

    // Defensive: not the `job` subcommand, and an argv too short to
    // carry a post-subcommand token, both keep the tail reparseable.
    let argv = os(&["camel", "run", "doc.yaml", "--", "x"]);
    assert_eq!(tail_terminator_boundary(&argv, &os(&["x"])), None);
    let argv = os(&["camel", "job"]);
    assert_eq!(tail_terminator_boundary(&argv, &[]), None);
}
