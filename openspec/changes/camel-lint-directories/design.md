# Design: camel-lint-directories

## Approach

Directory mode is additive and lives entirely in
`crates/camel-cli/src/commands/lint.rs`.

**Dispatch.** `run()` resolves the user-supplied path with
`std::fs::metadata` (follows symlinks). A directory enters `run_lint_dir`;
anything else (including dangling and special entries) enters the unchanged
`run_lint` path. A missing root exits 2 exactly like a missing file today.

**Walker.** `collect_lint_targets(root: &Path) ->
(Vec<PathBuf>, Vec<(PathBuf, std::io::Error)>)`: depth-first pre-order over
`std::fs::read_dir`, each directory's entries sorted by file-name `OsStr`
bytes; `read_dir` failures on subdirectories are collected and returned so
the caller can report them on stderr, count them as failed, and raise the
exit bucket to 2. Eligible extensions: `.yaml`, `.yml`, `.json` — matched
case-sensitively, like the glob defaults used elsewhere. An entry is a
candidate only when `std::fs::metadata` (follows symlinks) reports it as a
regular file — so symlinked files are linted through their target, FIFOs
and other non-regular entries are ignored (no hang), and a dangling
eligible symlink is returned as a per-file read failure. Below the root the
walker prunes (a) entries whose name starts with `.` and (b) directories
named `target` (rc-l4tc rationale: cargo build artifacts ship `*.json`
fingerprints); it never descends into symlinked directories. The root
itself is exempt from pruning, so `camel lint target/x/` still works. The
walk terminates on symlink cycles because cycles require descending a
symlinked directory.

**Engine.** `production_engine()` runs once per invocation, before any file
is read; failure exits 2 with a CLI-error message.

**Per file.** Reuse the shared predicates, in this order:
1. `camel_dsl::discovery::is_reserved_document` → skipped: info line on
   stdout (same wording as single-file mode), counted, not linted. The
   mission term "fixture/reserved document" means exactly this ADR-0062
   suffix predicate; no skip is based on directory names.
2. `std::fs::read_to_string` failure (missing target, permission,
   non-UTF-8, dangling symlink) → `error:` line on stderr, counted as
   failed, exit bucket 2. The walk continues so one bad file cannot hide
   the rest of the report. Subdirectory `read_dir` failures from the
   walker join this bucket.
3. Otherwise lint via the same per-file helper `run_lint` uses (including
   the `is_stagec_exempt_path` R-MOCK exemption, evaluated on the
   as-typed joined path — never canonicalized) and render diagnostics with
   ariadne, `file_id` = the walked path.

**Non-route documents are linted, not sniffed.** A parseable non-route
document (e.g. `package.json`) wraps as a bare single route and yields
R-SCHEMA errors — identical to naming that file individually today.
Directory mode deliberately keeps this single-file semantic: no shape
heuristic decides what gets linted, because silent skips would hide R-SYN
errors on genuinely broken route files. The user controls the blast radius
by choosing the root. The spec documents this consequence.

**Aggregation and exit codes.** Exit code is the maximum of: 2 when the
root is missing, engine construction fails (both abort before the walk and
print no summary), any per-file read fails, or a subdirectory cannot be
read; 1 when at least one error-severity diagnostic was emitted anywhere;
0 otherwise. Single-file mode keeps its exact today semantics (0/1/2
unchanged).

**Output streams.** Diagnostics (ariadne) and `error:` lines → stderr;
`skipped:` info lines and the summary → stdout. Directory mode prints
exactly one summary line, `linted N file(s), M skipped, F failed,
K diagnostic(s)` — N counts files read and linted (failed excluded),
K counts diagnostics of all severities — with no timings, so repeated runs
are byte-identical. The zero-target warning fires when the collected set
is empty before the reserved filter (a reserved-only directory does not
warn) and exits 0 unless a higher bucket applies. Single-file mode prints
no summary.

**Corpus walker.** `tests/lint_corpus.rs` keeps its own walker. Reuse is
rejected: the corpus gate carries corpus-specific policy the CLI walker
must not absorb — hidden entries are included there, it roots at two trees
(`examples/` and `crates/**/tests/fixtures`), it excludes
`examples/validator/schemas/`, and it feeds a baseline-compare contract.
Forcing the two together would couple a CI gate to CLI output rules.
Follow-up bd will record this. The CLI walker is a self-contained function
with no CLI-output coupling, so a future reconciliation stays cheap.

## Affected crates

- `camel-cli`: `src/commands/lint.rs` (dispatch, walker, `run_lint_dir`,
  summary), tests under `crates/camel-cli/tests/`.
- Docs: `docs/src/cli/index.md`, `docs/src/getting-started/cli.md`.

No changes to `camel-lint`, `camel-dsl`, or any other crate.

## Architecture boundaries

Control-plane CLI concern only. The lint engine and rules are untouched;
`camel-dsl::discovery` keeps its glob-pattern API and is consumed only via
the existing `is_reserved_document` re-export; `camel-api` reserved-suffix
predicates remain the single source of truth (ADR-0062). No data-plane
surface changes.

## Alternatives considered

- `glob` crate `**` walker: rejected — no per-entry symlink control,
  hidden-file semantics depend on match options, and it would duplicate the
  corpus walker's shape instead of replacing it.
- `walkdir` dependency: rejected — a ~40-line `std::fs` recursion does not
  justify a new dependency.
- Exclude JSON from directory walks: rejected — bd rc-61fgd says JSON where
  the lint already supports it; consistency with single-file mode wins and
  the R-SCHEMA consequence is documented.
- Route-shape sniffing before linting: rejected — silent skips hide real
  R-SYN diagnostics on broken route files.
- Per-file read failure exits 1: rejected — CI must distinguish an
  incomplete run (2) from a lint failure (1).

## Alternatives rejected by pre-flight expert (e_opus, adopted)

Exit-code precedence table must name the subdirectory `read_dir` failure;
pruning is below-root only; symlink rules written explicitly; extension
matching case-sensitivity stated; ordering function named exactly; output
stream per line type stated; summary carries a failed count.

Refs: ADR-0062, route-lint spec (`camel lint` CLI requirement, path-aware
reserved-suffix skip), rc-l4tc, bd rc-61fgd.
