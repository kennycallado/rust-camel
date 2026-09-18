# Proposal: redactstrict

## Why

bd rc-jskbc (e_glm stage-4, mission 112, finding 5): the `lint-log-redaction`
gate's redemption logic is name-based and trivially gameable. Any identifier
merely *named* `redacted_url` (holding a raw value) redeems a violation, and a
message-literal `{url}` capture is redeemed by a redact-named identifier
*anywhere* in the macro — even one sanitizing a different value. The gate is an
accident-guardrail today; it must require call-shape `redact*(...)` redemption
to resist adversarial or accidental bypass. `LOG_REDACTION_SENSITIVE` also
lacks `endpoint`, `address`, `host`, `remote` — field names that name the
value semantically (the mission-112 corpus hand-fixed `endpoint = %url` sites;
the `url` value ident is caught, but an `endpoint`-named raw local is not).

## What Changes

- Redemption predicate in `redaction_violation_reason`
  (`scripts/xtask/src/main.rs`): a segment is redeemed only by a **call-shape**
  redact — an identifier containing `redact` immediately followed by its
  parenthesized argument group (covers free fn `redact_url(&u)`, path fn
  `camel_api::redact::redact_url(u)`, method `u.to_redacted_string()`).
- Message-capture (`{url}` in a message literal) redemption becomes
  **binding-local**: only the segment that binds the captured ident
  (`url = <expr>`) can redeem, via call-shape. An implicit capture (no binding
  segment) is a violation — a redact call elsewhere cannot sanitize it.
- `LOG_REDACTION_SENSITIVE` and the `capture_re` alternation gain exactly:
  `endpoint`, `address`, `host`, `remote` (exact-match, leaf-or-standalone).
- Corpus audit: the stricter rules are run over the full workspace; every
  newly-caught site gets an explicit disposition (wrap via `camel_api::redact`
  helper, or a justified allowlist entry for provably credential-free values
  such as a bound `SocketAddr` IP). List extension lands atomically with all
  site fixes — the CI gate must never go red on main.
- Doc-comment updates in `lint_log_redaction` describing span-local
  redemption and the extended set.

Excluded (documented blind spots, unchanged): `span!`/`*_span!` field sets,
aliased macro imports, non-tracing sinks, prose word matching in message
literals (detection stays `{ident}`-capture-regex only — NOT substring word
match, which would false-positive on ordinary English like "endpoint created").

## Acceptance criteria

- Red tests first: raw `redacted_url` ident no longer redeems; `map(redact_url)`
  fn-pointer no longer redeems; `{url}` capture not redeemed by an unrelated
  redact ident in another segment; implicit `{url}` capture always flagged.
- All redemption paths (free fn / path / method call) still redeem.
- Existing 11 lint tests stay green (minus updated expectations where the
  contract tightened).
- `cargo xtask lint-log-redaction` reports 0 violations on the full corpus
  after remediation.
- Gates: `cargo fmt --all --check`, `RUSTC_WRAPPER= cargo clippy -p xtask
  --all-targets -- -D warnings`, xtask test suite, the other xtask lints green.

## Risk budget

Acceptable: tighter lint surfaces real leaks in the corpus (they are leaks by
rule definition) — fixed in-branch, atomically with the list extension.
Out of bounds: red CI on main (atomic landing), any change to escape-hatch
semantics (`// allow-log-redaction`, allowlist file), prose word matching,
value-flow analysis beyond the macro span.

Bd: rc-jskbc
