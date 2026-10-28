# Design: xtask-walkdir-errors

## Approach

House pattern is `lint_no_deprecated` (mission 433): inside the walk loop,

```rust
let entry = entry.map_err(|e| {
    let affected = e
        .path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| root.display().to_string());
    format!("cannot walk {affected}: {e}")
})?;
```

We extract that into one shared helper so the 5 sibling modules stop
hand-rolling (or skipping) it:

```rust
// scripts/xtask/src/walk.rs
/// Fail-closed walk: map every walkdir error to a lint-fatal message.
/// Message shape matches `lint_no_deprecated` (mission 433).
pub fn propagate_walk_errors<I>(
    root: &Path,
    it: I,
) -> impl Iterator<Item = Result<walkdir::DirEntry, String>>
where
    I: Iterator<Item = walkdir::Result<walkdir::DirEntry>>,
{
    it.map(move |e| {
        e.map_err(|err| {
            let affected = err
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| root.display().to_string());
            format!("cannot walk {affected}: {err}")
        })
    })
}
```

Call-site shape (composes with existing `filter_entry`/`follow_links` chains,
preserving entry order and every existing filter):

```rust
for entry in crate::walk::propagate_walk_errors(
    &root,
    WalkDir::new(&root).follow_links(false).into_iter(),
) {
    let entry = entry?;
    // ... unchanged body
}
```

Because the adapter sits at the END of the iterator chain (where
`.filter_map(|e| e.ok())` sat today), ordering, filters, and scan roots are
untouched — happy-path output is byte-identical.

Per-site notes (5 swept sites, 5 files; 2 parked):

| File | Site | Notes |
|---|---|---|
| `lint_cancel_tokens.rs` | ~86 | no `exists()` guard → missing root becomes `Err` (433 house behavior); loop in `Result` fn → `?` |
| `lint_component_deps.rs` | ~39 | keeps `if !components_dir.exists()` guard; loop in `Result` fn → `?` |
| `lint_context_citations.rs` | ~937 | violation-discovery walk in `Result` fn → `?`; keeps `if !dir.exists()` guard |
| `lint_metric_labels.rs` | ~407 | loop in `Result` fn → `?` |
| `lint_single_source.rs` | ~34 | keeps `exists()` guard; loop in `Result` fn → `?` |

Parked sites (mission Required #4 — fail-open dependence, do NOT change):

| File | Site | Why parked |
|---|---|---|
| `lint_context_citations.rs` | ~123 (`parse_crate_items`) | documented contract: "best-effort cross-reference, not a hard parser… silently skipped" |
| `lint_context_citations.rs` | ~1043 (`collect_crate_src_dirs`) | same symbol-table mechanism feeding that cross-reference; parked for consistency pending an owner ruling |

Missing scan root: lints with an `if !dir.exists()` guard
(`lint_component_deps`, `lint_single_source`, `lint_context_citations`)
keep returning `Ok` for a missing root — the guard is preserved and the
walk only runs when the root exists. `lint_cancel_tokens` has no guard, so
a missing `crates/components` becomes a hard error — identical to
`lint_no_deprecated`'s `walk_error_is_error_not_ok` test. On the real tree
every scan root exists, so gates are unaffected.

## Affected crates

- `xtask` (`scripts/xtask`): new `walk.rs` module (declared in `lib.rs` and
  `main.rs` following the existing module pattern), 5 lint modules edited.

## Architecture boundaries

Control-plane tooling only (`scripts/xtask`). No runtime, DSL, component,
service, language, or function crate is touched. No public API outside the
xtask binary/lib.

## Tests (deterministic injected error)

- `walk::propagate_walk_errors` unit tests:
  - injected error: `WalkDir::new(<nonexistent root>)` yields `Err` → helper
    surfaces `Err` containing `cannot walk` (deterministic, no chmod needed);
  - happy path: temp tree with nested dirs/files → helper yields exactly the
    same entries in the same order as the raw walkdir iterator.
  - `#[cfg(unix)]` unreadable-dir test: temp tree with `chmod 000`
    subdirectory, guarded to skip when running as root (euid 0) since root
    ignores mode bits.
- One representative end-to-end: `lint_cancel_tokens(tempdir_without_crates)`
  returns `Err` containing `cannot walk` (mirrors the 433 house test;
  `lint_cancel_tokens` is the touched lint with no `exists()` guard, so a
  missing root is a real walk error).
- CLI-level (integration test, `scripts/xtask/tests/walk_error_cli.rs`):
  synthetic workspace (Cargo.toml with `[workspace]`, empty `bridges/`,
  ratchet file) that passes root discovery and the ratchet read, missing
  `crates/components` → binary exits 1 with `error: cannot walk …` naming
  the affected path on stderr. Proves requirement 1 end to end (message +
  exit 1), not just the library `Err`.

## Alternatives considered

- `process::exit(1)` inside the helper on first error: rejected — not
  unit-testable and bypasses the existing `Result` plumbing.
- `panic!` on walk error: rejected — exit code 101, noisy, not a "clear
  `file: error` message with exit 1".
- Per-module copy of the 433 `map_err` block (no shared helper): rejected —
  7 duplicated sites invites the next lint to re-swallow; mission prefers a
  shared helper.

## Phases

Omitted — single-phase change (helper + 5 mechanical applications + tests).
