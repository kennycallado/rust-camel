# Design: deps-majors-minijinja-ed25519

## Approach

This is a **verification-and-evidence change**: the two major upgrades were
migrated on main by mission 490; mission 515 closes rc-k93qw's two
self-contained items by proving the landed state satisfies the acceptance
criteria and by writing down the one blocker that cannot be fixed locally.

Verification strategy, per dependency:

**minijinja 3.0.5.** The language boundary rules (`language-value-boundary`
spec: UTF-8 text in, typed conversion error out; strict-undefined render
errors) must not have moved. The replacement for the removed
`Value::from_serialize` in `engine.rs` is the `minijinja::value::Serde`
wrapper plus `Value::from_object`. The design calls for a **branch-by-branch
coverage audit** producing a persisted matrix
(`shape-matrix.md` in this change dir) mapping every context-build branch in
`engine.rs` to a named test: body non-stream → `Serde(&body_view)`; body
Stream → error value carrying `STREAM_REFUSAL`; headers map →
`Serde(&headers)`; properties map → `Serde(&properties)`; outer context map →
`Serde(&map)`; `header(key)`/`property(key)` readers → `Serde(v)` hit and
`Value::from(())` miss; `has_header(key)`/`has_property(key)` →
`Value::from(bool)`; plus the max-context-size error path and strict-undefined
render errors. Note: `Value::from_object(ContextReader)` serves **header/
property access and presence checks only** — it is not a streaming-body
mechanism. Any branch without a named test gains one.

**Exact-diagnostic pins.** Textual identity to the 2.24 diagnostics was
established by mission 490's landed migration (commit `54ad37c97`); no 2.24
diagnostic artifact exists in-repo, and regenerating one would require
unlocking the dependency graph, which this mission forbids (below). The
honest oracle is therefore a **forward-looking pin**: canonical broken-template
inputs (undefined path in interpolation and conditional, reader misuse —
wrong arg count / non-string arg, stream-body refusal, max-context-size
exceed) each get an `assert_eq!` on the full current `err.to_string()` text,
committed as the regression contract for future bumps. The park records this
basis precisely.

**ed25519-dalek 3.0.** ADR-0083 fixes the detached Ed25519ph envelope: the
148-byte layout (verifying key / signature / checksum / terminal magic
offsets) is the artifact format; RFC 8032 signing is deterministic, so the
same key and prehash must produce the same 64-byte signature under dalek 2.2
and 3.x. The committed fixture
`crates/camel-cli/tests/fixtures/ed25519ph_envelope_dalek_2_2.hex` is a
frozen envelope produced with 2.2.0 carrying its provenance header;
`signing_compat_test.rs` asserts verify + re-sign byte-identity under 3.x.
Run it green. If it fails, the mission **stops and parks with the byte
diff** — a format change is a separate decision, per the order.

**Transitive duplicate evidence.** `cargo tree -d` shows `ed25519-dalek
2.2.0` solely because `surrealdb-core 3.2.4` requires `jsonwebtoken ^10.3.0`
(resolver rejected `--precise 11.1.0`, which `camel-auth` already uses and
which does not pull ed25519-dalek 2.x). Removing it requires an upstream
surrealdb-core release on jsonwebtoken 11 — out of bounds. Evidence text is
captured verbatim in the park; rc-k93qw stays open with this note.

**Locked-verification policy.** The dependency graph is frozen for the whole
mission: `Cargo.toml` manifests, `Cargo.lock`, and the frozen signing fixture
must be byte-identical at close to their state at base `fdddcd3ee`
(verifiable via `git diff fdddcd3ee...HEAD -- Cargo.toml '**/Cargo.toml'
Cargo.lock 'crates/camel-cli/tests/fixtures/ed25519ph_envelope_dalek_2_2.hex'`
being empty). The jsonwebtoken resolver experiment above was executed
read-only (it failed and altered nothing) and is captured as evidence; no
further resolver experiments run against the working graph. Exception: a
`--dry-run` resolver reproduction is permitted — it invokes the resolver for
the error text but never writes `Cargo.lock`. The only
permitted diff is: this change's openspec artifacts, new/extended test files,
and the park.

## Affected crates

- `camel-template`: no code change; suites are the acceptance harness; may
  gain a test for an uncovered call-site shape.
- `camel-language-minijinja`: no production code change; owns the boundary
  semantics tests; gains the exact-diagnostic pin tests and any missing
  shape tests in `tests/`.
- `camel-cli`: no code change; `signing_compat_test` is run as evidence.
- `Cargo.lock` / manifests / frozen fixture: must remain byte-identical
  (locked verification).

## Architecture boundaries

No boundary moves. `camel-language-minijinja` keeps the language boundary
contract (UTF-8 text, typed conversion error, strict undefined) behind
`MinijinjaLanguage`; `camel-template` keeps template-set/reload behavior;
`camel-cli` keeps the ADR-0083 signing envelope byte-stable. The audit and
any new tests live entirely inside the existing crate seams — Languages
(engine semantics), Components (template suites), CLI (signing evidence).

## Affected ADRs and specs

- ADR-0083 (artifact signing envelope) — constrains byte-identity; unchanged.
- `openspec/specs/language-value-boundary/spec.md` — must remain satisfied.
- `openspec/specs/template-component/spec.md` — template suites reference.

## Phases

Single-phase: the work is one coherent verification slice (audit → run →
evidence → park). No milestone grouping is warranted at this size; tasks.md
has no `## Phase N` headings.
