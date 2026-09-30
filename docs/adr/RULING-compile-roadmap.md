# RULING: Compile roadmap (deployment-equivalence north star)

- **Status:** Recovered. Ruled 2026-09-12, recorded 2026-09-30. The ruled epic (`rc-rye74`) closed complete on 2026-09-30.
- **Ruling date:** 2026-09-12
- **Ruled by:** e_opus (expert escalation), compile-roadmap pre-flight
- **Epic:** `rc-rye74` (EPIC compile-roadmap: converge camel compile on deployment-equivalence)
- **Companion decisions:** ADR-0075 (self-contained executable artifact format, ruled the same day, epic `rc-wp01c`), ADR-0083 (R4 artifact signing)
- **Provenance:** recovered. See "Recovery note".

## Context

`camel compile` v1 (change `cli-compile`, epic `rc-wp01c`) landed single-document zero-toolchain artifacts on 2026-09-12. The same day e_opus ruled on the follow-on roadmap. The ruling was to land with v1 as `openspec/changes/cli-compile/RULING-compile-roadmap.md`. The ruling file lived only in the `camelcompile` worktree. That worktree went away before the file reached main. The archived change `2026-09-12-cli-compile` does not carry it. The ruling substance survives in full in the bd `rc-rye74` description. This document recovers it into `docs/adr/` so the north-star decision stays citable.

## Recovery note

The epic record preserves two internal section anchors. The five traps were "ruling section 2" and the honesty check was "ruling section 4". The original section numbering of the other parts is not recorded. This document therefore uses named sections instead of numbers. Canonical formulations (the north-star statement, bucket names, trap names) carry over unchanged. Every bucket item and trap below was cross-checked against ADR-0075 and its amendments. Where the epic record gives only a trap name, one grounding sentence cites the landed decision.

## Ruling: north star

NOT command-parity. Deployment-equivalence at the DOCUMENT level: any route/job document that runs correctly under `camel run` or `camel job` in a deployment posture MUST, when compiled, run identically sealed. Dev-loop capabilities are a category error inside a sealed unit.

## Ruling: scope boundary, three buckets

The MUST-NOT list is the epic's most valuable content. It stops scope creep.

**1a. MUST CONVERGE:**

- multi-route-file documents (`routeFiles` and includes)
- config embedding (`Camel.toml` plus profiles plus `[jobs]`)
- deploy-time asset embedding (certs, CA, keys, WASM, XSLT-XSD, SQL, static dirs, literal secrets)
- multi-entry jobs
- long-running route-server artifacts
- cross-target compile

**1b. MUST-NOT (permanent non-goals, never converge):**

- watch and hot-reload
- runtime file discovery and globbing
- ambient `Camel.toml` (config only via embed)
- wide argument surface (artifact stays `--report`, `--help`, `--version`, `--manifest`, plus the R4 `--verify`)
- compile-time `CAMEL_*` overrides

**1c. OPTIONAL (converge only if demanded):**

- payload compression (R6)

## Ruling: five architectural traps (original section 2)

1. **Embed-vs-discovery semantics.** What embeds at compile time and what the runtime may still discover must stay disjoint. The sealed artifact never falls back to filesystem discovery (ADR-0075 runtime wall).
2. **Asset confinement.** Reuse the `rc-0ks57` file-ancestor-confinement discipline. Prefer no materialization at all.
3. **Manifest schema evolution.** Use a separate `manifest_schema` field. NEVER reuse the trailer version byte. The trailer version describes framing only (ADR-0075, v2 amendment).
4. **Trailer, strip, and signing interplay.** An artifact is final and immutable. A strip or mutation invalidates the trailer, and a signature must cover the complete final bytes. Signing runs only after the final embed (ADR-0083).
5. **The 16 MiB cap.** It must become configurable at R2, not before. R2 replaced the fixed constant with `--max-payload-bytes` (ADR-0075, R2 amendment).

## Ruling: roadmap order (fixed dependencies)

- R1 multi-document embed is the KEYSTONE. It unlocks R2 assets and R3 multi-entry.
- R2 embeds assets, then the format finalizes, then R4 signs. Never sign before the final embed.
- R5 route-server artifacts are independent of R1.
- R7 cross-target is LAST. It is the one true architectural break: the compiler cannot copy `current_exe` for another target.

## Ruling: honesty check (original section 4)

V1 has ZERO regret-blocks:

- text payload does not block AOT
- the single-doc store generalizes to multi-doc
- host-copy does not block cross-target
- BLAKE3 does not block signing

The reject-loud wall is a forward-compatible inventory with built-in acceptance oracles.

## Acceptance

The epic closes only when both hold:

- Every 1a item has a landed child or a recorded principled deferral.
- Every 1b item is documented as permanent in the `cli-compile` spec prose (R0).

## Disposition (epic outcome, 2026-09-30)

The epic closed complete. R0 through R5 landed on main:

- R0, permanent non-goals in spec prose: `f3da5731`, archive `ce0ae07c`
- R1, multi-document virtual store, plus R2, deploy-asset embed: `9b5cfc2a`
- R3, multi-entry jobs: `0031410e5`
- R4, ed25519 signing: `f174c3d1e`, keypin follow-up `d8badd35` (ADR-0083)
- R5, route-server artifacts: `124b354e9`

Recorded principled deferrals: R7 cross-target (`rc-73eal`, P3, sequenced last by this ruling) and the R6 optional bucket (`rc-9iexz`). Evidence follow-up `rc-z332y` (gRPC and WS batteries) stays open. The permanent non-goals live in `openspec/specs/cli-compile/spec.md`.

## References

- bd `rc-rye74` (the epic. Its description carries the surviving full text of this ruling)
- bd `rc-pwqz9` (this recovery)
- ADR-0075, self-contained executable artifact format
- ADR-0083, artifact signing envelope
- openspec change `cli-compile` (archived 2026-09-12)
- `openspec/specs/cli-compile/spec.md` (permanent non-goals prose)
