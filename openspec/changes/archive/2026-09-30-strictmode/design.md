# Design: strictmode

## Approach

A **strict directive** in the truststore file: a line whose
whitespace-separated token set is exactly `strict` (leading/trailing
whitespace tolerated, matching how pin lines tokenize; case-sensitive). It is
the only strictness surface — no CLI flag.

**Codec (`compile/trust.rs`).** `TrustStore` gains `strict: bool`. In
`TrustStore::parse`, a line tokenizing to the single token `strict` sets the
flag; repeated `strict` lines are idempotent (unlike duplicate pins,
identical directives carry no contradiction — managed fragments may
concatenate). Any other use (`strict` with a second token) falls through to
the existing malformed-entry rule. `strict` cannot collide with a pin (pins
start with `blake3:`). The verbatim `lines` retention is untouched — a floor
rewrite preserves directive lines like comments.

**Policy (`compile/runtime.rs`).** The single hook is `trust_policy_rejection`
— both verify sites already funnel through it, so the surfaces cannot drift:
boot and `--verify` under either store source (flag or `CAMEL_TRUSTSTORE`),
plus the boot-side dispatch of `--manifest`/`--help`/`--version`, which reach
the hook only with an env-supplied store because the `--truststore` flag is
argument-surface-rejected for those modes. Today the `manifest.signing ==
None` branch returns `None` before touching the store (the accepted ADR-0083
boundary). The change: that branch now parses the supplied store. If the
store carries the strict directive, print the `strict-unsigned` diagnostic
and return a new `TrustRejection::StrictUnsigned` (exit 2). If the directive
is absent, return `None` exactly as today. Strict requires a signing block
and changes nothing else — pin and freshness decisions for signed artifacts
stay exactly as under the same store without the directive (a pinned
schema-4 artifact without a floor still passes, per the migration rule).
Signed manifests with an absent envelope keep the more specific
`truststore-pin` strip-rule diagnostic — the strict check never fires for
them because the signing block is present.

**Fail-closed decision (grilled).** Parsing the store for unsigned artifacts
means a malformed store now fails closed (`truststore-parse`) for unsigned
artifacts too, where today it booted (the store was never read). Fail-open is
unacceptable for a security directive — an attacker able to corrupt the store
file must not be able to silently disable strict mode by making it
unparseable. The "behavior unchanged" guarantee is therefore scoped to
well-formed stores: a valid store without the directive leaves the unsigned
path byte-identical (parse succeeds, flag false, proceed).

**Zero-cost guard rails.** No truststore supplied → `truststore?` returns
before any read (R4 path unchanged, zero new reads). Unsigned + supplied
store adds exactly one store parse. Envelope-present paths are untouched.
Unsigned compiles stay schema 3 and byte-identical; the compile side never
reads the truststore (`CAMEL_TRUSTSTORE` stays benign).

**Directive-only rationale.** The ADR-0083 amendment made the truststore the
deployment-owned policy artifact: pins and floors travel in one file the
operator controls. A per-invocation `--strict` flag would fragment that
policy (forgetting the flag reopens the hole) and a strict flag without a
truststore is meaningless — strict rejection with an empty pin set verifies
nothing. The bd acceptance criteria name "flag or truststore directive";
the directive is chosen as the strictly stronger design.

## Affected crates

- `camel-cli`: `compile/trust.rs` (directive parse + `strict` field + unit
  tests), `compile/runtime.rs` (`StrictUnsigned` variant, unsigned branch of
  `trust_policy_rejection`, integration tests in `tests/compiled_artifact_test.rs`).

## Architecture boundaries

CLI-runtime concern only (compiled-artifact argument surface), inside the
existing hexagonal boundary: no Runtime/DSL/Component/Service surface moves.
The codec change is additive to the truststore parser; the policy change is
one branch in the shared policy helper both verify sites already call. Docs:
`docs/src/cli/compile.md` truststore section, ADR-0083 amendment note,
canonical `cli-compile` spec delta (MODIFIED pin requirement + ADDED strict
requirement). CONTEXT-MAP.md checked for a truststore clause at
implementation time.
