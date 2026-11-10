# Proposal: hardened-client-audit

## Why

Two ops reports were the same defect class: a hardening constant silently overriding configured
timeout behaviour while error text named the wrong thing (`rc-9pfby`, fixed in `f862b0f2b`;
`rc-jpcqt`, open). The workspace has many more outbound HTTP stacks (reqwest builders, lettre
SMTP, async-imap, tonic gRPC, opensearch-rs, rmcp streamable-http, opentelemetry-otlp,
tokio-tungstenite). Nobody has ever enumerated their bounds in one place, so the next
"timeout_secs=90 died at 30s" incident is a matter of time. This change is the audit that
answers, for every client: what constant bounds what, is it documented, and does the error text
honestly name the limit that fired.

bd: `rc-s8itn` (P2). Not a refactor mission.

## What Changes

- One complete audit table (crate, builder site, constant, what it bounds, documented?, error
  honesty) covering every outbound HTTP client in `crates/**`, recorded in `design.md`.
- One bd issue per real finding, priority by blast radius, each quoting its table row, all
  `discovered-from: rc-s8itn`.
- `rc-jpcqt` re-verified against current code: substance still accurate; its quoted connect
  value is stale (code has 30s connect / 60s io, issue says 60s connect); a comment records
  this. `rc-9pfby` (closed) linked, not duplicated.
- Explicitly excluded: `camel-component-llm/**` (owned by mission 560 — read as reference
  shape only), test-only builders, config plumbing, non-HTTP stacks (redis-rs, MQTT).

## What Does NOT Change

- No code fixes land. The bounded-fix clause in the mission order permits unambiguous,
  behaviour-neutral error-text fixes; the audit found zero wrong-limit messages, so no
  error-text fix is selected. Every vague-text and undocumented-cap finding is either a
  docs gap or a design decision (new config keys, new deadlines) and becomes an issue
  instead.
- No dependency changes, no behaviour changes, `camel-component-llm/**` untouched.

## Acceptance criteria

- `design.md` table covers every production client found by the enumeration command quoted in
  the design; exclusions listed with reasons.
- Every finding has a bd issue with the row quoted; no duplicates of `rc-9pfby`/`rc-jpcqt`.
- `rc-jpcqt` accuracy statement recorded on the issue itself.
- Zero source-file changes: `git diff --name-only` against base shows only
  `openspec/**` and fleet inbox files.

## Risk budget

- Low: report-only change. Risk is incompleteness (a missed client stack) and misreading a
  constant's semantics. Mitigated by quoting `file:line` for every row so the next audit can
  re-verify mechanically, and by the enumeration command being reproducible.

## Affected crates

None modified. Audited: camel-component-keycloak, camel-auth, camel-function,
camel-component-mcp, camel-mail, camel-http, camel-opensearch, camel-cxf, camel-otel,
camel-component-grpc, camel-ws, camel-component-surrealdb, camel-platform-kubernetes,
camel-container, camel-function container provider (bollard), camel-bridge (plus its XSLT,
JMS and XSD-validator RPC stacks), camel-component-llm (read-only reference).
