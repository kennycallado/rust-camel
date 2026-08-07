# Tasks: audit-fix-docdrift-t1-baseline

Single-phase. Five independent per-crate tasks; each is independently
dispatchable to a worker. No ordering constraint between them.

## camel-api

### Task 1: Fix phantom rustdoc variant + remove stale API-006 TODO

**Files:**
- `crates/camel-api/src/claim_check.rs` (modified)
- `crates/camel-api/src/lib.rs` (modified)

**Steps:**
1. In `crates/camel-api/src/claim_check.rs`, locate the `ClaimCheckRepository`
   trait definition. Read the actual method signatures for `get`,
   `get_and_remove`, and `pop` — they return `Result<Message, CamelError>` (a
   not-found condition is an `Err`, not `None`).
2. Inspect `crates/camel-api/src/error.rs`: confirm no `NotFound` variant exists
   (only `ComponentNotFound`, which is semantically wrong for a claim-check
   payload and MUST NOT be cited as the replacement).
3. Correct the 5 phantom references in the trait-level rustdoc (the `# Contract
   (C1)` block) and the per-method `///` doc-comments at approximately lines 20,
   25, 36, 41, 52. Replace each `Err(CamelError::NotFound(...))` with a generic,
   accurate not-found contract description (e.g. "Returns an error if the key
   does not exist.") — do NOT name a specific `CamelError` variant, since none
   maps cleanly to a claim-check payload miss.
4. In `crates/camel-api/src/lib.rs`, delete the two-line comment at lines 7–8
   (`// TODO(API-006): Consider re-exporting Component, Endpoint, Consumer,
   Producer ... unified API surface.`). The rationale already exists in lines
   5–6 ("Note: Component, Endpoint, Consumer, Producer traits are defined in
   `camel-component-api`. This crate focuses on data types and EIP
   abstractions."). Do NOT add a re-export.

**Tests:**
- `no_phantom_notfound_variant`: source after change → `rg -n 'CamelError::NotFound' crates/camel-api/` → expected: 0 matches.
- `no_api006_todo`: source after change → `rg -n 'API-006' crates/camel-api/src/` → expected: 0 matches.
- `api_lib_tests`: `cargo test -p camel-api --lib` → expected: pass (no behavior change; doc-only).
- `api_clippy`: `cargo clippy -p camel-api -- -D warnings` → expected: exit 0.

**Acceptance:**
- `rg 'CamelError::NotFound' crates/camel-api/` exits non-zero (0 matches).
- `rg 'API-006' crates/camel-api/src/` exits non-zero (0 matches).
- Rustdoc references resolve to a real variant or use a generic not-found
  description (no phantom `NotFound` variant, no `Option`).
- `cargo fmt --check` and `cargo clippy -p camel-api -- -D warnings` clean.

- [x] 1

## camel-config

### Task 2: Remove three stale TODO(CONFIG-004) comments (hot-reload is wired)

**Files:**
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/src/context_ext.rs` (modified)

**Steps:**
1. In `crates/camel-config/src/config.rs`, delete the comment line
   `// TODO(CONFIG-004): hot-reload watch plumbing not fully implemented yet.`
   at approximately line 19 (above the `pub watch: bool` field). Keep the
   accurate doc-comment above it ("Enable file-watcher hot-reload. Defaults to
   false. Can be overridden per profile in Camel.toml or via `--watch` /
   `--no-watch` CLI flags.").
2. In the same file, delete the two comment lines above `pub watch_debounce_ms`
   at approximately lines 45–46:
   `// TODO(CONFIG-004): Hot-reload via file watcher not yet implemented.`
   `// watch_debounce_ms is parsed but currently unused.`
   Replace them with an accurate one-line doc-comment:
   `/// Debounce window (ms) for the file-watcher hot-reload; consumed by `camel run`.`
   (Verified consumed at `crates/camel-cli/src/commands/run.rs:507`.)
3. In `crates/camel-config/src/context_ext.rs`, delete the comment line
   `// TODO(CONFIG-004): config.watch flag is parsed, but hot-reload wiring is
   not implemented here yet.` at approximately line 143 (inside
   `configure_context`). The wiring is implemented in the CLI run path.
4. Do not alter the `watch` / `watch_debounce_ms` field types, serde attributes,
   or the builder methods — comment-only change.

**Tests:**
- `no_config004_todo`: source after change → `rg -n 'CONFIG-004' crates/camel-config/src/` → expected: 0 matches.
- `config_lib_tests`: `cargo test -p camel-config --lib` → expected: pass.
- `config_clippy`: `cargo clippy -p camel-config -- -D warnings` → expected: exit 0.

**Acceptance:**
- `rg 'CONFIG-004' crates/camel-config/src/` exits non-zero (0 matches).
- `watch_debounce_ms` field and its builder method unchanged.
- `cargo fmt --check` and `cargo clippy -p camel-config -- -D warnings` clean.

- [x] 2

## camel-dsl

### Task 3: Sync README step-coverage list with shipped step variants

**Files:**
- `crates/camel-dsl/README.md` (modified)

**Steps:**
1. Read the authoritative step enumeration:
   `crates/camel-dsl/src/model.rs` (`pub enum DeclarativeStep` at ~line 532).
   This is the user-facing step set the README documents; collect its full
   variant list. (`RouteDslStep` in `route_ast.rs` and `DeclarativeStepKind` in
   `contract.rs` are internal/sanity-only — do NOT add their variants to the
   README list.)
2. In `crates/camel-dsl/README.md`, find the "All step types" list at
   approximately line 24 (currently: `to, log, set_header, set_body, transform,
   filter, choice, split, aggregate, delay, wire_tap, multicast, recipient_list,
   stop, script, bean, throttle, load_balance, dynamic_router, routing_slip,
   do_try`).
3. Compare the README list against the enum variant set from step 1. Add any
   shipped step variant missing from the README list; remove any README entry
   that is not a real shipped variant. Keep the list in a stable order.
4. Do not edit any code; this is a README-text-only change.

**Tests:**
- `readme_covers_shipped_steps`: after change, every one of the 36
  `DeclarativeStep` variants in `crates/camel-dsl/src/model.rs` (To, SetHeader,
  SetHeaderIfAbsent, SetProperty, SetBody, ConvertBodyTo, DynamicRouter, Filter,
  Function, LoadBalance, Log, Choice, Split, Aggregate, WireTap, Multicast,
  RoutingSlip, RecipientList, Stop, Throttle, Script, StreamCache, Marshal,
  Unmarshal, Validate, Bean, Delay, Loop, Enrich, PollEnrich, IdempotentConsumer,
  ClaimCheck, Sampling, Sort, Resequence, DoTry) appears, converted to
  snake_case (PascalCase → snake_case: e.g. `SetHeaderIfAbsent` →
  `set_header_if_absent`, `IdempotentConsumer` → `idempotent_consumer`,
  `PollEnrich` → `poll_enrich`), on the README "- **All step types**:" line
  (≈L24). Concrete check, scoped to that line only:
  `for v in To SetHeader SetHeaderIfAbsent SetProperty SetBody ConvertBodyTo DynamicRouter Filter Function LoadBalance Log Choice Split Aggregate WireTap Multicast RoutingSlip RecipientList Stop Throttle Script StreamCache Marshal Unmarshal Validate Bean Delay Loop Enrich PollEnrich IdempotentConsumer ClaimCheck Sampling Sort Resequence DoTry; do snake=$(echo "$v" | sed -E 's/([a-z])([A-Z])/\1_\2/g; s/([A-Z]+)/\L\1/g'); grep -F "All step types" crates/camel-dsl/README.md | grep -qw "$snake" || echo "MISSING: $snake"; done`
  → expected: no `MISSING:` output.
- `dsl_lib_tests`: `cargo test -p camel-dsl --lib` → expected: pass (unchanged).

**Acceptance:**
- Every variant of `DeclarativeStep` in `model.rs` appears in snake_case form
  on the README "- **All step types**:" line.
- No phantom (non-shipped) step remains on that line (e.g. `transform` is not a
  `DeclarativeStep` variant and must be removed).
- No source file other than `crates/camel-dsl/README.md` is modified.

- [x] 3

## camel-cli

### Task 4: Sync README command/flag listing; remove stray Overview + dead PROC-004 citation

**Files:**
- `crates/camel-cli/README.md` (modified)
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. Read the `enum Commands` at `crates/camel-cli/src/main.rs` line 16 (and the
   full variant list through its closing brace). The variants are `Run`, `New`,
   `Journal`, `Plugin`, `Openapi` (plus clap-derived `help`). This is the source
   of truth for command names; the README renders them lowercase.
2. In `crates/camel-cli/README.md`, the top-level `Commands:` block
   (approximately lines 14–18) lists `new, run, journal, help` and omits
   `plugin` and `openapi`. Add `plugin` and `openapi` rows. Source each
   one-line description from the doc-comment immediately above the matching
   variant in `main.rs` (the variants carry `///` doc-comments, not
   `#[command(about = "...")]` attributes).
3. In the same README, find the stray `## Overview` header at approximately
   line 37. The text under it ("Command-line interface for Apache Camel in
   Rust." followed by `--template`/`--profile-layout`/`--force` options) belongs
   to the `camel new` subsection above it. Remove the `## Overview` header line
   so the options render as part of `## \`camel new\``. Do not delete the option
   lines themselves.
4. In the README `## \`camel run\`` section (approximately lines 84–92), the
   options list already documents `--routes`, `--config`, `--watch`,
   `--no-watch`, and `--health-port` — verified present. It is MISSING the
   OpenTelemetry flags. Add the three OTel flags read from the `Run { ... }`
   variant in `main.rs`: `--otel` (enable OTel export), `--otel-endpoint <URL>`
   (OTLP endpoint, implies --otel), `--service-name <NAME>` (implies --otel),
   each with the one-line description from its doc-comment. Do NOT re-add
   `--watch`/`--no-watch`/`--health-port` (already present).
5. In `crates/camel-cli/CONTEXT.md`, the `## Metrics` section at lines 30–32
   contains `See TODO(PROC-004) in individual processor crates for the broader
   instrumentation gap.` Remove the dangling `TODO(PROC-004)` pointer. Rewrite
   the sentence to state the current metrics status without a work-item
   reference (e.g. "Metrics instrumentation for CLI commands is not yet wired;
   processor-crate instrumentation is tracked separately."). Verify the
   rewrite is accurate and self-contained.

**Tests:**
- `readme_lists_all_commands`: scoped to the top `Commands:` block
  (≈README L14–18), each command appears individually. Concrete per-command
  checks (each must match):
  `sed -n '/^Commands:/,/^$/p' crates/camel-cli/README.md | grep -qw 'plugin'`
  `sed -n '/^Commands:/,/^$/p' crates/camel-cli/README.md | grep -qw 'openapi'`
  `sed -n '/^Commands:/,/^$/p' crates/camel-cli/README.md | grep -qw 'new'`
  `sed -n '/^Commands:/,/^$/p' crates/camel-cli/README.md | grep -qw 'run'`
  `sed -n '/^Commands:/,/^$/p' crates/camel-cli/README.md | grep -qw 'journal'`
  → expected: every command exits 0 (a match).
- `no_stray_overview`: `rg -n '^## Overview$' crates/camel-cli/README.md` →
  expected: 0 matches.
- `readme_lists_otel_flags`: scoped to the `## \`camel run\`` options block, each
  OTel flag appears individually. Concrete per-flag checks (each must match):
  `sed -n '/^## .camel run./,/^## /p' crates/camel-cli/README.md | grep -F -- '--otel-endpoint'`
  `sed -n '/^## .camel run./,/^## /p' crates/camel-cli/README.md | grep -F -- '--service-name'`
  `sed -n '/^## .camel run./,/^## /p' crates/camel-cli/README.md | grep -Fw -- '--otel'`
  → expected: each exits 0.
- `no_proc004_pointer`: `rg -n 'PROC-004' crates/camel-cli/CONTEXT.md` →
  expected: 0 matches.
- `cli_clippy`: `cargo clippy -p camel-cli -- -D warnings` → expected: exit 0.

**Acceptance:**
- README top `Commands:` block includes `plugin` and `openapi`.
- No `## Overview` header remains in the README.
- README `camel run` options include `--otel`, `--otel-endpoint`, `--service-name`.
- `CONTEXT.md` has no `PROC-004` reference.
- Only the two listed files are modified.

- [x] 4

## camel-builder

### Task 5: Correct version-stale "canonical v1" strings to v2 + coupled test assertions

**Files:**
- `crates/camel-builder/src/lib.rs` (modified)
- `crates/camel-builder/tests/canonical_spec_test.rs` (modified)

**Steps:**
1. In `crates/camel-builder/src/lib.rs`, replace every `"canonical v1"` literal
   with `"canonical v2"`. There are 11 occurrences at approximately lines 963,
   965, 982, 988, 1049, 1054, 1072, 1077, and the inline-test assertion
   substrings at lines 3318 (comment), 3319, 3467. Also update the comment at
   line 3318 ("rejected in canonical v1" → "rejected in canonical v2").
2. In `crates/camel-builder/tests/canonical_spec_test.rs`, update the two
   assertion substrings at lines 42 and 82 from `contains("canonical v1 does
   not support step ...")` to `contains("canonical v2 does not support step
   ...")`.
3. Do not change which steps are rejected, the rejection control flow, or any
   non-version text. Only the version token `v1` → `v2` moves. This is a
   diagnostic-text compatibility change; rejection semantics are unchanged.

**Tests:**
- `no_v1_strings_remain`: `rg -n 'canonical v1' crates/camel-builder/` →
  expected: 0 matches.
- `v2_strings_present`: `rg -n 'canonical v2' crates/camel-builder/src/lib.rs`
  → expected: ≥1 match.
- `builder_tests`: `cargo test -p camel-builder` → expected: all pass
  (assertions updated in lockstep).
- `builder_clippy`: `cargo clippy -p camel-builder -- -D warnings` → expected:
  exit 0.

**Acceptance:**
- `rg 'canonical v1' crates/camel-builder/` exits non-zero (0 matches).
- `cargo test -p camel-builder` passes (the canonical_spec_test and inline
  assertions track the v2 strings).
- No rejection decision or non-version text changed.
- `cargo fmt --check` clean.

- [x] 5
