# Design: redactstrict

## Approach

All logic stays inside `redaction_violation_reason` in
`scripts/xtask/src/main.rs` — the token-walk architecture (span-bounded syn
collection via `MacroSpanCollector`, top-level comma segmentation,
leaf-or-standalone sensitive matching) is unchanged; only the redemption
predicates and the sensitive set tighten.

1. **Call-shape redemption.** Replace `seg_has_redact` (any ident whose
   lowercase contains `redact`) with `seg_has_redact_call`: true iff some
   token pair `(Ident, Group)` in the segment has the ident containing
   `redact` (case-insensitive) and the group delimiter `Parenthesis`. This
   covers free-fn (`redact_url(&u)`), path (`camel_api::redact::redact_url(u)`
   — the last path ident is adjacent to the group), and method
   (`u.to_redacted_string()`) shapes; a bare ident `redacted_url` and a
   fn-pointer `map(redact_url)` no longer redeem. Redemption is
   **span-local**: a local pre-redacted outside the macro (`let url =
   redact_url(&u); debug!(url = %url)`) is NOT recognized — wrap at the call
   site or use an escape hatch. This mirrors `lint-secrets`' single-span
   contract.
2. **Binding-local message-capture redemption.** Delete `macro_has_redact`.
   For each sensitive `{ident}` capture in a message literal, locate the
   segment binding that ident (`ident = <expr>` at segment start); the capture
   is redeemed iff that segment satisfies `seg_has_redact_call`. No binding
   segment (implicit capture of a local) → violation: the capture binds the
   raw local and no call in this macro can sanitize it. Zero implicit
   sensitive captures exist in the corpus today (pre-flight scan), so this
   tightens without live blast radius.
3. **Sensitive-set extension.** Append `endpoint`, `address`, `host`,
   `remote` to `LOG_REDACTION_SENSITIVE` and to the `capture_re` alternation
   (both must move in tandem). Matching stays exact-ident,
   leaf-or-standalone — `remote_addr`, `host_name`, `endpoint_id`,
   `config.topic` (object position qualifying a benign leaf) are NOT caught;
   `host = %self.host`, `%endpoint`, `config.host` (sensitive leaf),
   `"{host}"` are. `capture_re` remains an
   `{ident}`-capture regex — never a substring word match (prose
   false-positive guard, per pre-flight: "direct endpoint created"-class
   literals must not fire).
4. **Corpus audit + remediation.** Run the tightened lint over the workspace
   in the worktree. Each newly-flagged site gets exactly one disposition:
   (a) wrap the value with a `camel_api::redact` helper (helpers pass
   credential-free URLs/IPs through per ADR-0076 strictest-wins), or
   (b) an allowlist entry with a `#` justification comment, only when the
   value is provably credential-free (e.g. a bound `SocketAddr` IP) and
   wrapping is semantically wrong. Pre-flight blast-radius estimate:
   ~12–15 true sites after prose filtering (camel-ws, camel-redis,
   camel-grpc, camel-http, services/otel+prometheus hot spots).

## Affected crates

- `xtask` (scripts/xtask): predicate rewrite, list extension, tests,
  doc comments. No public API.
- Corpus remediation sites (expected: camel-ws, camel-redis, camel-grpc,
  camel-http, services/*): log-call edits only, no connection behavior
  change — wrapped URL values pass through unchanged when credential-free,
  except query fragments, which are sentinel'd per ADR-0076 strictest-wins
  (e.g. redis `?db=N` logs as `?[redacted]`).
- `scripts/xtask/allowlist-log-redaction.txt`: justified entries only.

## Architecture boundaries

Tooling-side change living in the assurance layer (xtask), orthogonal to
Runtime/DSL/Components/Services/Languages. Corpus site edits touch only
diagnostic log lines, not control flow. Authorities: ADR-0076 (URL redaction
strictest-wins — dynamic host/address/endpoint/remote values are treated as
leaks; credential-free values pass helpers unchanged), ADR-0051 (credential
redaction at diagnostic boundaries). The `event!(Level::X, ...)` first
segment (`Level::DEBUG` path tokens) must survive segmentation and the new
predicate — regression-tested.

## Phases

### Phase 1: Call-shape + binding-local redemption

- **Goal:** redemption cannot be gamed by names or by unrelated segments.
- **Dependencies:** none. Zero expected corpus delta (live sites already use
  call-shape `redact_*(...)`).
- **Externally-visible types/interfaces:** none (internal predicate).
- **Deliverable:** red tests → predicate rewrite → green suite + doc-comment
  update, one commit.
- **Exit-criteria:** new unit tests cover bare-ident bypass, fn-pointer,
  unrelated-segment capture redemption, implicit capture, and all three legit
  call shapes; `lint-log-redaction` green on full corpus; fmt + clippy
  `-p xtask --all-targets -D warnings` clean.

### Phase 2: Sensitive-list extension + atomic corpus remediation

- **Goal:** `endpoint/address/host/remote` exact-match coverage with the CI
  gate staying green.
- **Dependencies:** Phase 1 (call-shape predicate — sites must redeem via
  calls, not names).
- **Externally-visible types/interfaces:** allowlist entries with
  justification comments.
- **Deliverable:** list + `capture_re` extension, every newly-caught site
  fixed (wrap or justified allowlist), one atomic commit.
- **Exit-criteria:** `lint-log-redaction` reports 0 violations on the full
  workspace corpus; each allowlist entry carries a justification; fmt,
  clippy, xtask test suite, and the other xtask lints green.

## Alternatives considered

- **Value-flow analysis** (track locals bound from redact calls): rejected —
  disproportionate for a token-walk lint; span-local contract documented
  instead.
- **Macro-wide call-shape redemption for captures:** rejected — reintroduces
  exactly the bypass rc-jskbc closes (a redact call on a different value
  would mask the capture).
- **Substring/prose word matching for new names:** rejected — pre-flight
  found English-prose false positives ("endpoint created", "MCP remote
  '{name}'"); detection stays `{ident}`-capture-only.
- **Separate new lint command:** rejected — same span, same escapes, same
  corpus; one gate keeps CI wiring and allowlist surface unified.
