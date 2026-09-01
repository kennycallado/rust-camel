# Design: fuzzing-mutation-tooling

## Approach

One excluded `fuzz/` crate plus one xtask wrapper. The wrapper, not the
developer, owns every safety property: target-dir isolation, worktree
enforcement, toolchain checks, and crash minimization.

- Harness: `fuzz/fuzz_targets/dsl_yaml.rs` is a thin
  `libfuzzer_sys::fuzz_target!(|data: &[u8]| ...)` body. It converts bytes
  with `str::from_utf8` (invalid UTF-8 returns early), then calls
  `camel_dsl::yaml::parse_yaml_with_threshold_and_security` (entry point at
  `crates/camel-dsl/src/yaml.rs:188`). The invariant is "never panic":
  malformed input must produce an `Err`, never a panic or a hang. The
  harness passes `camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD`
  (`128 * 1024`) and `SecurityCompileContext::default()`, the exact values
  the production loaders use.
- Wrapper: `cargo xtask fuzz <target> [--time N]` in `scripts/xtask`
  (new `fuzz.rs` module, new `Commands::Fuzz` variant in the clap enum).
  It performs, in order: worktree guard, toolchain guard, seed copy,
  then `cargo +nightly fuzz run <target> <corpus-dir> --
  -max_total_time=<N|60> -artifact_prefix=<worktree>/target-fuzz/artifacts/<target>/`
  with `CARGO_TARGET_DIR` set to `<worktree>/target-fuzz`. The artifact
  prefix is mandatory: without it libFuzzer writes crash files to
  `fuzz/artifacts/` inside the repository, which breaks the isolation
  guarantee.
- Worktree guard: `git rev-parse --git-dir` versus
  `git rev-parse --git-common-dir`. Equal paths mean the main checkout;
  the command refuses with a one-line reason. This is the same linked
  worktree property the conductor flow relies on.
- Toolchain guard: probe `cargo +nightly fuzz --version`. On failure print
  the install hint (`cargo install cargo-fuzz` plus rustup nightly) and
  exit non-zero without building.
- Seeds: committed under `fuzz/seeds/dsl_yaml/`. The wrapper copies them
  into `target-fuzz/corpus/dsl_yaml/` before the run when the corpus dir is
  empty or missing. Seed sources: malformed-route inputs already present in
  camel-dsl tests (`schema_validation.rs`, `format_aware_errors.rs`), plus
  hand-written alias-bomb, billion-laughs-style anchor, and deep-nesting
  YAML files from the audit's regression set.
- Crash path: the wrapper creates `target-fuzz/artifacts/<target>/` before
  the run and snapshots its file list. When libFuzzer exits non-zero and
  the diff against the snapshot shows a new artifact, the wrapper invokes
  `cargo +nightly fuzz tmin <target> <artifact>` with the same
  `CARGO_TARGET_DIR` as the run, prints the minimized input path (located
  by globbing the artifacts directory for files the tmin run created; the
  exact naming is verified by the Phase 2 tmin drill), and prints
  instructions to promote the input into a
  committed `#[test]` regression case. Because of the artifact prefix, no
  crash file can land in `fuzz/artifacts/`; raw crash files stay under
  the git-ignored `target-fuzz/`.

## Affected crates

- Root `Cargo.toml`: add `"fuzz"` to `workspace.exclude`. No member glob
  matches `fuzz/` today, but `examples/*`-style glob growth makes the
  explicit exclude the defense the decision doc requires.
- `fuzz/` (new, excluded): `Cargo.toml` (name `camel-fuzz`, edition
  `2024`, `rust-version = "1.89"` matching the workspace, deps
  `libfuzzer-sys` plus path deps `camel-dsl` and `camel-api` — the latter
  supplies `DEFAULT_STREAM_CACHE_THRESHOLD`), `fuzz_targets/dsl_yaml.rs`,
  committed `fuzz/Cargo.lock` and seeds. Exclusion keeps the root
  `Cargo.lock` untouched.
- `scripts/xtask`: new `src/fuzz.rs`, extended `Commands` enum and dispatch.
- `.gitignore`: the `/target-fuzz/` entry. It covers build output, corpora,
  and crash artifacts; no `fuzz/artifacts/` entry is needed because the
  artifact prefix makes that path unreachable.

## Architecture boundaries

No runtime, data-plane, or DSL behavior changes. The harness consumes
`camel-dsl` as a read-only path dependency, the same public API
configuration loaders call. Control plane, data plane, and component
boundaries are untouched. The only new public surface is the xtask CLI
subcommand, which is developer tooling outside QUALITY GATES.

## Alternatives considered

- `proptest` instead of cargo-fuzz: rejected. No coverage guidance, and the
  decision doc (already blessed by e_opus) selected cargo-fuzz.
- Direct `cargo fuzz` invocation without an xtask wrapper: rejected. The
  wrapper is the only layer that enforces the cold-main disk policy and the
  worktree guard.
- Committing the grown corpus: rejected for Phase 1. Only curated seeds are
  committed; grown corpora stay local under `target-fuzz/`.
- Corpus outside the repository tree (`/tmp` or `$XDG_CACHE_HOME`, the
  adoption decision's letter in §3.3): rejected as the default. A
  worktree-local git-ignored `target-fuzz/corpus/` satisfies the same
  binding properties (never committed, purgeable with the worktree) and
  keeps runs reproducible per-worktree; the deviation from the decision's
  suggested location is deliberate.
