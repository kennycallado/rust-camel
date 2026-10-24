# Design: scenario-absent-assertions

## Approach

Pre-flight expert ruling (e_opus, 2026-10-24): option (b) — an explicit absence matcher, `extract` untouched. Null-on-absent (option a) is rejected because it merges "absent" with "present carrying a null value" (the header map already admits `Value::Null`) and turns a typo'd selector into a silent pass; any per-extract opt-out (`?`, `optional: true`) inherits the same ambiguity against `expectation: null`. Keeping `extract` strict is the compatibility argument: existing suites rely on the hard failure to catch typos, and the mission forbids silently turning a typo into a pass.

Three coordinated pieces:

1. **`Expectation::Absent` (camel-matchers).** Additive variant on the `#[non_exhaustive]` enum (ADR-0049). Grammar: an object whose single key is `absent` with a null payload parses to it; any other payload is a load error ("`absent` takes no argument"), mirroring `exists`/`ignore`. A bare string `"absent"` keeps parsing as literal `equals "absent"` — the dual grammar already gives the escape. API bridge: `expectation_matches` keeps its `(&Expectation, &Value)` signature — `Absent` fails closed on every materialized value (absence is not a value property), and the none-projection never reaches it. The runner owns the `Option<Value>` projection and evaluates the pairing itself: `(Absent, None)` passes, `(Absent, Some(_))` fails, `(other, None)` fails, `(other, Some(v))` delegates to `expectation_matches`. No shared-signature break.
2. **`select` on the `lastReceived` target (camel-integration-test).** The object target form gains an optional `select: "<selector>"`. At run time the target's subject is `select_from(message, selector)` — `None` when the source is absent. `Absent` passes iff the projection is `None`; a `Some` value fails with a mismatch naming the selector and the observed KIND only (`present (string)`), never the content — an absence diagnostic is a leak diagnostic and scenario output is shared (credential-safe by construction, ADR-0051 spirit; a sentinel regression test pins that a leaked token value never appears in the failure text). A non-absent expectation paired with `select` is a load error naming the restriction (v1 scope: `select` exists solely for absence; the null-value case asserts through `extract` → variable → `equals: null`, which exists today). The last-received message is resolved exactly as today (`vars.last_received`), so a validate on an endpoint that received nothing still fails with the existing "no message has been received" error — absence never passes on a missing subject.
3. **Load-time selector validation.** The validated selector grammar is where typo protection stays real for the new surface: a `select` head must be `headers` or `body` AND carry a non-empty sub-path. Bare `body`/`headers` always resolve (`Some`), so they can never prove absence — load error. Scalar heads `status`/`method`/`path` always resolve (absent scalars already map to `Value::Null`, runner.rs:1343-1351) and their sub-paths are outside the grammar (`status.why`) — load error. Unknown heads (`header.X`) — load error. `absent` on any other target (variable, partner, sql, surreal, redis) or on `lastReceived` without `select` — load error. Because `expectation_from_value` is shared, registering `absent` as a matcher key would make `{absent: null}` parse in every grammar position that reuses it (`send.expectReply`, partner `requests` body expectations, row cells); each of those positions therefore rejects an `Absent` expectation at load with the same restriction message — the parser-wide registration plus per-position rejection keeps one verb parser (rc-qvz6) without silent literal-object fallbacks. `extract` is NOT touched: its selector grammar, runtime hard failure, and error text stay byte-identical.

Case table (normative for spec + docs):

| Case | `extract` | `select` + `{absent: null}` |
|---|---|---|
| Absent header | hard failure (unchanged) | passes |
| Present header, null value | yields null into the var | FAILS (observed kind `present (null)`); equals-null asserts via extract → variable target |
| Misspelled head (`header.X`) | runtime hard failure (unchanged) | load error |
| Misspelled header name / body path | runtime hard failure (unchanged) | indistinguishable from absence → absent passes; docs state this honestly and recommend a positive control (assert a header known to be present) |
| Absent response body (null/empty) | bare `body` yields null (unchanged) | bare heads rejected at load; `body.<path>` on a null/empty body → `None` → passes |
| Body leaf mapped to null (`{"a": null}`) | `body.a` yields null (unchanged) | FAILS — present-null is not absent |
| Endpoint never received a message | n/a | fails with the existing no-message error |

Migration note for docs: nothing behavioral migrates — `extract` keeps failing on absent sources. What migrates is the workaround: suites extracting the whole headers object + regex allowlist (camel-112) simplify to one `select` + `absent` assertion.

## Affected crates

- `camel-matchers`: `Expectation::Absent` variant, matcher-key registration (`absent`), fail-closed `expectation_matches` arm (absent never matches a materialized value).
- `camel-integration-test`: `{absent: null}` parse in `document/validate.rs` (`expectation_from_value`), `select` field on the `lastReceived` target parse + load-time selector validation, `lastReceived` projection arm in `runner.rs` (`validate_action`), `render_expectation_mismatch` arm, README grammar reference.
- docs: `docs/src/testing/scenario-documents.md` (worked example, case table, migration note).

## Architecture boundaries

Testing crates consume the engine, never the reverse (ADR-0069 §10): the matcher variant lands in `camel-matchers` (already a `camel-integration-test` dependency); no core/processor change. `#[non_exhaustive]` posture (ADR-0049) covers the enum growth. The vocabulary ban, provisioning gate, and step handlers are untouched. Redaction law (ADR-0051): the absent-mismatch diagnostic is credential-safe by construction — it names the selector and the observed KIND only and MUST NOT render the observed value's content, deviating deliberately from the other expectation mismatch renderers because an absence diagnostic is a leak diagnostic and scenario output is shared.

## Alternatives considered

- **(a) extract yields null on absent** — rejected: conflates absent with present-null, converts head AND name/path typos into passes, and the only real protection left (head validation) cannot catch name/path typos; the mission requires typo protection to survive intact.
- **(b') absent matcher without `select`** — rejected: the `lastReceived` subject is the body only, so a header absence claim would have no subject to attach to; `select` is the minimal projection that lets the matcher resolve where the source lives.
- **Generalized `select` over all expectations** — deferred: useful, but beyond what absence needs (mission non-goal); the load error naming the restriction keeps the door open without promising grammar.
- **Per-extract `optional: true` / `?` suffix** — rejected with (a): makes `expectation: null` ambiguous and moves the typo trap into the author's per-line diligence.
