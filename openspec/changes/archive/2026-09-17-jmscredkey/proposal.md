# Proposal: jmscredkey

## Why

Mission redactconv (bd rc-yvjp3, landed d69cfa71) ruled the
key-position credential echo under e_opus OPTION B and recorded the
ruling in ADR-0076 appendix "Key-position credential-shape symmetry".
The ruling closed the channel in code: `camel_api::redact::
redact_url_with_query_allowlist` renders a credential-shaped key as a
bare `<redacted>`, and camel-api pins the behavior
(`allowlist_suppresses_credential_shaped_key_*`).

The spec canon never caught up. `openspec/specs/jms/spec.md` still
says "the rendered key keeps its original encoded bytes" with no
qualifier. Under that clause a denylist-matched pair such as
`?user%3Asecret%40host=1` would render `user%3Asecret%40host=
<redacted>` and echo the embedded credential in the key position.
The landed code is spec-compatible only because it over-masks, and
over-masking is safe (ADR-0051). e_opus forbade an in-worktree
spec-canon edit: the fix is this OpenSpec change (bd rc-tfugr).

## What Changes

- MODIFIED requirement "Broker URL redaction keeps benign query keys
  and masks credential-shaped values" in `openspec/specs/jms/spec.md`:
  the key-rendering clause gains the key-position credential-shape
  exception, worded from ADR-0076 appendix. All six existing
  scenarios carry over unchanged.
- Two new scenarios pin the exception: a credential-shaped key
  (percent-encoded in both hex cases, and literal) never echoes, and
  non-shaped sensitive keys keep their authored key bytes.
- Two camel-jms pin tests fix the exception at the component
  boundary (`redact_broker_url`), matching the existing exact-output
  pin pattern in `crates/components/camel-jms/src/config.rs`: one
  for the masked credential-shaped key, one for the kept non-shaped
  key names.
- At archive time, one sentence in the ADR-0076 appendix §2 spec
  note records the landing, so the note stays true once the canon
  carries the qualifier.
- No production-code change. `camel-api` behavior and its pins
  landed in mission redactconv. No schema change.

## Acceptance criteria

- `openspec validate jmscredkey --type change --json` reports no
  delta-structure errors.
- The amended requirement states the exception: a denylist-matched
  pair whose raw key's single-pass minimal decode contains `@` AND
  (`:` OR `//`) renders as a bare `<redacted>`.
- At least one scenario pins the credential-shaped-key masking, and
  one pins the non-shaped counterpart (key bytes kept).
- The camel-jms pin test passes:
  `RUSTC_WRAPPER= cargo test -p camel-component-jms` green,
  `cargo fmt --check` and `cargo clippy -p camel-component-jms --
  -D warnings` clean.
- The delta is blessed (e_gpt spec-bless) and reviewed (r_glm)
  before archive.

## Risk budget

Spec text plus two tests. No runtime behavior changes, so the leak
surface cannot move. The only accepted risk is wording drift from
the ADR-0076 ruling: the delta quotes its predicate verbatim
(`@` AND (`:` OR `//`), minimal-decode triple, hex case-insensitive).
Out of bounds: any edit to the denylist entries, the 256-byte cap,
the fragment sentinel, or the userinfo window rules.
