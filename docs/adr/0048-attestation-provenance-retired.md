# ADR-0048: HMAC attestation provenance — RETIRED

**Status**: Retired (2026-07-26)
**Supersedes**: Original ADR-0048 (HMAC-SHA256 attestation provenance)

## Decision

Retire HMAC-SHA256 attestation signing/verification. Replace with plain
`.bless.json` files containing `{verdict, hash, expert}` in text plain.

## Rationale

The HMAC threat model was incoherent for this project's topology:

1. **Workers share the conductor's environment.** All subagents inherit
   `$ATTESTATION_HMAC_SECRET` from the same devShell. A worker can compute
   a valid HMAC — the "only conductor can sign" assumption was false.

2. **The adversary doesn't exist.** Workers are cooperative LLMs executing
   the conductor's instructions, not malicious actors trying to bypass gates.
   Cryptographic provenance solves a problem that doesn't occur in practice.

3. **Ausence of human ≠ presence of attacker.** Autopilot mode (no human
   supervision) was used to justify the crypto. But autopilot = interactive
   without pauses, not an adversarial environment.

## What survives

- **`hash-artifacts`** (xtask): kept for drift detection. If artifacts change
  after blessing, the hash changes, and `/apply` can detect it. This is
  process integrity, not cryptographic security.
- **`.bless.json`**: plain JSON with `{verdict, hash, expert, kind}`.
  No HMAC, no secret, no plugin guard.
- **`.review.json`**: plain JSON with `{verdict, reviewer, impl_hash}`.

## What was removed

- `xtask sign-attestation` / `verify-attestation` commands
- `mod attestation` (HMAC-SHA256, constant-time comparison, RFC 4231 test)
- `.opencode/plugin/attestation-guard.ts` (runtime guard)
- CI `verify-attestation` step in quality gates
- `$ATTESTATION_HMAC_SECRET` in flake.nix and GitHub Actions
