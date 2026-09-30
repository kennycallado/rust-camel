# Proposal: strictmode

## Why

ADR-0083's truststore amendment (mission 294 `keypin`, d8badd35, bd rc-07psv)
accepted a documented boundary: the truststore does not apply to unsigned
(schema ≤ 3) artifacts. Pinning governs WHICH signing key is trusted, not
WHETHER signing is required. A deployment that pins keys still boots any
unsigned artifact untouched — an attacker who cannot forge a signature can
simply ship an artifact with no signature at all. bd rc-gwfgs tracks the
follow-up: a strict, opt-in refusal to boot unsigned artifacts while a
truststore is in force.

## What Changes

Add a **strict directive** to the deployment truststore: a line whose
whitespace-separated content is exactly the token `strict`. When the supplied
truststore carries the directive, an artifact with no signing block (schema 2
or 3, no envelope) fails closed at every trust-policy surface — boot and
`--verify` under either store source, and the boot-side dispatch of
`--manifest`/`--help`/`--version` under the `CAMEL_TRUSTSTORE` variable (the
`--truststore` flag is argument-surface-rejected for those modes) — with exit
2 and a `strict-unsigned` diagnostic. Strict requires a signing block and
changes nothing else: pin and freshness decisions stay exactly as under the
same store without the directive. The strip rule keeps its more specific
`truststore-pin` diagnosis for signed manifests whose envelope is absent.

Design is directive-only (no CLI flag), per the ADR-0083 amendment spirit:
the truststore is the deployment-owned policy artifact, so the policy travels
with the pins in one file rather than fragmenting into per-invocation flags.
A strict flag without a truststore would be meaningless (strict rejection
with no pin set verifies nothing) and is not added.

- Affected crates: `camel-cli` (truststore codec `compile/trust.rs`, trust
  policy in `compile/runtime.rs`, integration tests `tests/compiled_artifact_test.rs`).
- Docs: `docs/src/cli/compile.md` (truststore section), ADR-0083 amendment
  note, canonical spec `cli-compile`.
- Excluded: manifest schema changes (unsigned stays schema 3, byte-identical),
  compile-side behavior (`CAMEL_TRUSTSTORE` stays benign), envelope framing,
  any change to the no-truststore path.

## Acceptance criteria

- A truststore line whose whitespace-separated content is exactly the token
  `strict` parses as the strict directive (leading/trailing whitespace
  tolerated, as for pin lines); repeated `strict` lines are idempotent;
  `strict` with any second token stays a `truststore-parse` malformed line.
- With a strict truststore supplied, booting an unsigned (schema-3) artifact
  exits 2 with a `strict-unsigned` diagnostic and does not boot; `--verify`
  on the same artifact fails the same way; `--manifest`/`--help`/`--version`
  under a `CAMEL_TRUSTSTORE` strict store fail the same way, while the
  `--truststore <path>` flag stays rejected for those modes.
- Without the directive, behavior under a well-formed store is byte-identical
  to today: unsigned artifacts boot unchanged under a permissive truststore
  (default, back-compat). A supplied store that cannot be read or parsed
  fails closed for unsigned artifacts too (`truststore-parse`), where today
  it booted — an unreadable strict policy must never be silently skipped.
- Strict applies to schema-2 artifacts equally (no signing block).
- Signed-manifest-without-envelope under a strict store keeps the existing
  `truststore-pin` strip-rule diagnostic.
- The opt-in is documented in the truststore docs (`docs/src/cli/compile.md`).
- Tests cover strict-refusal and default-accept paths for boot and `--verify`.

## Risk budget

Low. The directive defaults absent (permissive); a well-formed store without
the directive leaves every existing path byte-identical, and the
no-truststore path never parses the store (zero new reads on the R4 chain).
One deliberate behavior change: a supplied store that cannot be read or
parsed now rejects unsigned artifacts too (`truststore-parse`), where today
it booted — fail-open on store corruption would let an attacker disable
strict mode by corrupting the file. Parsing risk is bounded: one new
exact-token match in `TrustStore::parse`, fail-closed on anything else. Out
of bounds: manifest schema bumps, new CLI flags, truststore lock/write
semantics changes.
