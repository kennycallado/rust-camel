# Design: ci-clippy-parity

## Approach

Three pieces, one commit.

**1. ci.yml sync (the bug fix).** The `quality` job's `clippy` step `run:` block
is rewritten to the registry command set. The workspace line becomes:

```
cargo clippy --workspace --all-features \
  --exclude camel-cli \
  --exclude camel-component-kafka \
  --exclude security-keycloak \
  --exclude security-wasm-policy \
  --all-targets \
  -- -D warnings
```

The other three invocations (`-p camel-component-kafka --all-targets`,
`-p camel-cli`, `-p camel-cli --no-default-features --features
flavor-regular,exec --all-targets`) already match the registry byte-for-byte and
stay. The existing dynamic-linking comment stays (it explains why excludes
exist); a new comment above names `AGENTS.md ## QUALITY GATES` as source of
truth, references bd rc-2s1vm, and says "change both in one commit" — same
pattern as mission 559's `docs.yml` doc-build comment (rc-o8pxk). No trigger,
timeout, matrix, permission, or other step changes.

**2. lint-gate-parity (the drift guard).** New `scripts/xtask/src/lint_gate_parity.rs`,
dispatched from `main.rs`/`lib.rs` like sibling lints. Static, no new deps, no
cargo builds:

- Parse the `AGENTS.md ## QUALITY GATES` fenced block with the same line-based
  approach `scripts/gates-local.sh` uses (regex on `- name:` / `run:` /
  continuation lines). No YAML library needed.
- Resolve each checked gate to its workflow step through an explicit mapping
  table — `clippy` → `ci.yml` / job `quality` / step `clippy`,
  `doc-build` → `docs.yml` / job `validate` / step `Build Rustdoc`. A gate
  missing from the registry or missing its mapped workflow step is a violation
  (the 561 hole in reverse: silent absence hides drift too). Parsing workflow
  jobs/step names is text-level: locate `- name: <step>` under the mapped job
  and take its `run:` block; the two mapped files are pinned by path so no
  general YAML parser is needed.
- Normalize each side into a list of statements: a line ending in `\` joins the
  next line (continuation); any other newline separates statements. Within a
  statement strip `#` comments, collapse whitespace runs, trim. Quotes and
  command separators are preserved verbatim, so rendered output is executable
  and paste-ready.
- Environment prefixes: a `NAME=value` statement prefix on the registry side
  (doc-build: `RUSTDOCFLAGS="-D warnings"`) is compared against the mapped
  workflow's effective environment for that step — step `env:` over job `env:`
  over workflow-level `env:` over unset. The remaining command text is compared
  statement-by-statement.
- Compare statement lists (ordered). Exit 1 with paste-ready output on drift:

  ```
  lint-gate-parity: MISMATCH gate 'clippy' (.github/workflows/ci.yml)
  expected (AGENTS.md registry):
    <statement 1>
    <statement 2>
    ...
  actual (.github/workflows/ci.yml 'clippy'):
    <statement 1>
    ...
  ```

  Environment mismatches print the expected effective value and the actual
  effective value (with its source: step/job/workflow env). Exit 0 with
  `lint-gate-parity: OK (2 gates in parity)` when clean.
- Unit tests in-module against small fixture strings (registry block snippet +
  workflow step/job snippets): parity passes; clippy drift fails with
  expected/actual; doc-build crate-set drift fails; missing mapped step fails;
  comment-only difference passes; a dropped command separator between the four
  clippy invocations fails (statement boundaries preserved); docs.yml-style
  inherited workflow env satisfies the registry prefix; missing or overridden
  workflow env fails with the effective-value pair.

**3. Registry add + sweep wiring.** One new entry appended to the
`AGENTS.md ## QUALITY GATES` fenced block: `- name: lint-gate-parity` /
`run: cargo xtask lint-gate-parity`. No existing line changes. This is the only
mechanism that makes `gates-local.sh` run it — the script parses the registry
and states so in its header ("a new gate is picked up here the moment it lands
there"). CI wiring of the new gate is explicitly out of scope (order forbids
other workflow changes); the lint only checks its two mapped gates, and
`lint-gate-parity` is not in the mapping, so it does not demand parity for
itself. Follow-up bd issue filed for
the CI step.

**Spec ruling (e_gpt, spec bless 2026-11-10):** `ci-compile-gate` is the
camel-test integration compile gate (`full-tests-linux` job) — the parity
requirement does NOT belong there. Approved placement: new capability
`ci-clippy-gate` (ci.yml mirrors the registry clippy command; sibling of
`ci-doc-gate`), new capability `gate-parity-lint` (the lint), and a MODIFIED
requirement in `ci-doc-gate` (its "Registry and workflow disagree" scenario
upgrades from review-only to mechanical enforcement). `ci-compile-gate` stays
unchanged.

## Affected crates

- `scripts/xtask`: new `lint_gate_parity.rs` + dispatch; no dependency changes.
- No workspace crate changes; `cargo build --workspace` unaffected.

## Architecture boundaries

No Runtime / DSL / Components / Services / Languages / Functions surface is
touched. The lint reads repo text files only (`AGENTS.md`,
`.github/workflows/*.yml`) and stays inside the existing xtask tool boundary.

## Risk / cost notes

- Widened clippy line proven on main by rc-e6kz2 (exit 0; one deprecation fix
  already landed).
- Parity lint cost: ~200 lines xtask + tests, runs in milliseconds.
- Registry add ruled in bounds (e_gpt, spec bless): one new entry, no existing
  gate value changes; primary interpretation approved, no fallback needed.
  Also approved: taking the optional lint; checked set {clippy, doc-build}
  with explicit mappings; lint's own CI wiring deferred to a bd follow-up.
