# ADR-0048: Attestation provenance via HMAC-signed blessings

## Status

Accepted

## Context

The expert-gated OpenSpec workflow gates implementation on a blessing:
`/opsx:apply` refuses to start unless `openspec/changes/<change>/.attestation.json`
has `verdict == "BLESSED"` and the recomputed artifact hash matches. The
analogous `.review.json` gates plan review.

The trust of an attestation derives entirely from **who wrote it**. That
provenance was unenforced prose ("NEVER write an attestation manually"). A
worker subagent (`@workers/w_fast`) has `write/edit/bash`, so nothing stopped:

```bash
echo '{"verdict":"BLESSED","hash":"sha256:..."}' > .../.attestation.json
```

This forges a blessing and bypasses the entire expert gate.

### Approaches considered

1. **Per-agent permission denial** (`edit: { "**/.attestation.json": "deny" }`).
   Rejected as a standalone: workers have `bash: true`, so `echo > file`,
   `tee`, `sed -i`, heredocs, and `python -c` all bypass edit/write rules.
2. **Plugin `tool.execute.before` interception.** Strong *runtime* control:
   can abort both edit/write and bash calls targeting attestation files, and
   can recover the calling agent (via the `chat.message` hook / SDK session
   lookup). But it is JS in the same opencode process the worker runs in, and
   it is **invisible to CI** — it cannot be the trust root.
3. **Restricted write-attestation script.** All agents share one shell and
   filesystem, so "only the conductor may run it" is unenforceable without an
   embedded secret — i.e. it collapses into approach 4 anyway.
4. **HMAC signature keyed by a conductor-only secret.** The attestation carries
   `hmac = HMAC-SHA256(secret, verdict|hash|expert)`. Verification recomputes
   it. Workers lack the secret, so they cannot produce a signature that
   verification accepts. Crucially this is **verifiable offline, in CI, by
   anyone, forever** — which is exactly where the real gate runs.

## Decision

**HMAC (approach 4) is the trust root; the plugin (approach 2) is a fail-fast
secondary layer. Permission denial (1) and the script (3) are dropped.**

Rationale: prevention and verification defend different threats. CI is the real
trust boundary, and only a cryptographic signature is enforceable there
independent of the runtime that produced the file. The plugin improves live UX
by failing forgery immediately, but a worker who disables the plugin still
cannot forge a valid HMAC — so removing the plugin degrades convenience, not
security.

## Mechanism

### Signing (conductor-light only)

```
ATTESTATION_HMAC_SECRET=<secret> \
  cargo run -p xtask -- sign-attestation \
    --change-dir openspec/changes/<change> \
    --verdict BLESSED --expert e_opus --kind bless
```

Writes `.attestation.json`:

```json
{
  "verdict": "BLESSED",
  "hash": "sha256:<canonical artifact hash>",
  "expert": "e_opus",
  "hmac": "<HMAC-SHA256(secret, 'BLESSED|sha256:...|e_opus')>",
  "alg": "HMAC-SHA256"
}
```

- **Canonical artifact hash**: SHA-256 over every file in the change dir
  (sorted relative-path order), each contributing `<relpath>\0<len>\0<bytes>`,
  excluding the attestation files themselves. Renames and content edits both
  change the hash.
- `--kind review` writes `.review.json` and expects verdict `APPROVE`.

### Verification (CI)

```
ATTESTATION_HMAC_SECRET=<secret> \
  cargo run -p xtask -- verify-attestation \
    --change-dir openspec/changes/<change> --kind bless
```

Exits non-zero on: missing/empty secret, missing file, **HMAC mismatch**
(forged or unsigned), wrong verdict, or **artifact drift**. Constant-time
comparison on the HMAC. This is the enforcement point `/opsx:apply` and CI call.

### Runtime guard (defense in depth)

`.opencode/plugin/attestation-guard.ts` blocks any non-conductor agent from
writing `.attestation.json` / `.review.json` via `edit`, `write`, or `bash`
(command-string scan). Fails closed on unknown agent identity.

## Secret management

- `ATTESTATION_HMAC_SECRET` is provided to the conductor-light's environment
  and to CI (as a masked secret). It is NOT in any worker/expert/reviewer
  environment and NOT committed.
- Rotating the secret invalidates all prior attestations (they must be
  re-blessed) — acceptable, since attestations are per-change and short-lived.

## Threat model

**Prevented**

- *Worker forges a blessing* (the stated primary threat): forged JSON fails
  HMAC verification in CI; the runtime guard also blocks it live.
- *Verdict tampering*: verdict is inside the signed tuple.
- *Artifact swap after blessing*: hash is re-checked; drift fails.
- *Cross-change replay*: hash binds the signature to that change's artifacts;
  it will not verify against a different change dir.
- *Expert-spoofing in the record*: `expert` is inside the signed tuple, so the
  recorded decider cannot be altered without breaking the HMAC.

**Remaining / out of scope**

- *Secret compromise*: anyone with `ATTESTATION_HMAC_SECRET` can mint valid
  attestations. This reduces the trusted surface to "who holds the secret"
  (conductor-light + CI) — the intended trust boundary. Mitigate by never
  exposing it to worker/expert/reviewer envs and rotating on suspicion.
- *Malicious conductor-light*: the conductor is defined as trusted; this ADR
  does not defend against the signer itself.
- *Same-change replay*: signing verdict X then re-signing verdict Y overwrites
  the file; both are legitimately signed. Not a forgery, just latest-wins.
- *Runtime guard bypass*: a worker can disable/delete the plugin, but cannot
  thereby produce a CI-valid attestation — the HMAC still fails. The guard is
  UX, not the trust root.

## Consequences

- New xtask verbs: `sign-attestation`, `verify-attestation` (both `--kind
  bless|review`). No new crate dependencies — HMAC-SHA256 is implemented over
  the existing `sha2` dep (RFC 4231 test vector asserts correctness).
- CI adds a `verify-attestation` step per gated change.
- Conductor prose changes from "write the file" to "run `sign-attestation`".
- One new plugin file under `.opencode/plugin/`.
