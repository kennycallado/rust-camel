# Proposal: keypin

## Why

The R4 signing surface (mission 278, ADR-0083) proves a self-contained
chain: the artifact bytes were signed by the holder of the private key
whose public key hashes to the manifest `key_fingerprint`. It proves
nothing about WHO that key holder is, and nothing about FRESHNESS:

1. **No external trust anchor.** An attacker who re-signs a trojan
   artifact under their own key gets a fully valid chain; the
   fingerprint merely changes. Operators pin by eyeballing the
   `--verify`/`--manifest` fingerprint against a known value — manual,
   error-prone, unenforceable.
2. **Rollback passes.** Swapping an OLDER `exe`+`.sig` pair signed by
   the SAME key verifies cleanly. A signature proves authenticity,
   not freshness (bd rc-07psv; ADR-0083 Consequences defers both).

## What Changes

- **Truststore (pinning):** a deployment-owned store of pinned key
  fingerprints, consulted at verify time (boot verify and `--verify`).
  The store never enters the artifact, the envelope, or the manifest —
  it is operator input, like the signing key. With a truststore
  present, a fingerprint outside the pin set fails closed, and a
  signed manifest without its envelope fails closed (strip attack).
  Without one, behavior stays exactly the R4 self-contained chain
  (compat decision per design.md).
- **Rollback freshness:** a signed artifact carries a signed
  monotonic freshness marker; the truststore records the floor per
  pinned key; verify rejects artifacts older than the floor. The
  exact carrier and rule follow the bd wording (key/serial policy)
  and are fixed in design.md + the ADR-0083 amendment.
- **ADR-0083 amendment section** (no new ADR): records the truststore
  and freshness decisions as an extension of the R4 lineage.
- **cli-compile spec:** ADD requirement blocks for the truststore and
  freshness surface; MODIFIED blocks carry every current scenario
  name (strict guard).

Excluded: key management (generation, rotation ceremonies), key
discovery/infrastructure (PKI), and any change to the 148-byte
envelope framing unless design.md proves it unavoidable.

## Acceptance criteria

- Pin-match: artifact signed by a pinned key verifies with a
  truststore present.
- Pin-mismatch: artifact whose fingerprint is not pinned fails
  closed, naming the truststore step, when a truststore is supplied.
- Rollback reject: an older signed artifact below the recorded floor
  fails closed when freshness policy applies.
- No-truststore behavior is unchanged from R4 (all existing r4sign
  scenarios pass untouched).
- Unsigned v1 compat untouched: no envelope, no truststore → boot as
  today.
- ADR-0083 carries an amendment section; gates green.

## Risk budget

Trust machinery: fail-closed on any malformed truststore or ambiguous
policy input; never weaken the existing chain. No key material in
logs. Format changes to frozen surfaces (envelope bytes, schema-3
unsigned manifests) are out of bounds. e_gpt consult budget: 1, only
if pinning/freshness semantics fork beyond the bd text.

## Affected crates

- `camel-cli`: `src/compile/signature.rs`, `src/compile/manifest.rs`,
  `src/compile/runtime.rs`, `src/commands/compile.rs`,
  `tests/compile_command_test.rs`, `tests/compiled_artifact_test.rs`.
- Docs: `docs/adr/0083-artifact-signing-envelope.md` (amendment).

Reference: bd rc-07psv (P3), epic rc-rye74 R4 follow-up; openspec
change `keypin`; ADR-0083; archived change `r4sign`.
