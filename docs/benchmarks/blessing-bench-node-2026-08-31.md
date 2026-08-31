# Merge Blessing — OpenSpec change `bench-node`

- **Authority:** e_opus (papal authority gate), invoked by the human owner.
- **Date:** 2026-08-31
- **Worktree:** `/home/shared/rust-camel-worktrees/bench-node`
- **Branch:** `feature/bench-node`
- **HEAD:** `f014685bfc53374c2c250c4bc22a8f29bd52e4f6`
- **Base (merge-base with main):** `84ad405d714f294fddc4cd0398a6eadd1ebca93a`
- **Commits:** 29 · **Tree at start and end of verification:** clean.

---

## 1. Evidence I EXECUTED (not read-trusted)

Every item below was run in the worktree on this host during the gate.

| # | Proof | Command (essence) | Result |
|---|-------|-------------------|--------|
| 1 | Explicit-list full dry-run | `bench run --scenarios=<7 active> --dry-run` | **exit 0**, `resolved cells: 52 (expected: 52)`, 14 node cell lines |
| 2 | Node cell count | grep `node-native\|node-fastify` over dry-run | **14** |
| 3 | Guard RED path | `mv` `xslt-bridge/node-fastify` → dry-run | **exit 1**, `error: contender family node declares completeness but is missing fixtures in: xslt-bridge/node-fastify`; restored; **tree clean** |
| 4 | t2-json input parity | `BENCH_PAYLOAD_BYTES=32768 node t2-json/node-native/route.mjs` | `BENCH_INPUT_SHA256=a0db69e1…eae10e9`, `BENCH_ROUTE_READY bytes=32781` |
| 5 | split-aggregate parity | `node split-aggregate/node-native/route.mjs` | `BENCH_INPUT_SHA256=123444b4…a159316`, `BENCH_ROUTE_READY items=100` |
| 6 | COVERAGE open-if count | `sed … COVERAGE.md \| rg -o '\? open-if' \| wc -l` | **14**; zero numeric measurements leaked in the section |
| 7 | README diet | `rg -n "M[1-4]…payload axis" benchmarks/README.md` | **0 hits** |
| 8 | Zone contract | `ls -A benchmarks/ \| grep -v gitignore \| sort` | exactly `README.md attic bench harness records runner scenarios` (7) |
| 9 | openspec validate | `openspec validate bench-node --type change --json` | `"valid": true` |
| 10 | fastify no-listen (protocol B) | per-fixture `rg app.ready() / .listen(` | 6/6 fixtures: `ready=2 listen=0`; `listen` present ONLY in http-server (both node cells) |
| 11 | Supply chain hygiene | `git ls-files … node_modules` / `… package-lock.json` | **0** node_modules committed, **9** lockfiles committed, `.gitignore:22` rule present |
| 12 | saxon SEF single-compile | read `xslt-bridge/node-native/route.mjs` | SEF compiled ONCE at startup (engine-init slot, pre-marker); per-tick uses `stylesheetInternal` — no recompile |
| 13 | pin.sh report | `pin.sh --report` | `NODE_VERSION=22.14.0`, `NODE_SHA256=9d942932…f0c2` (exit 0) |
| 14 | ARG↔pin drift parity | `rg ARG` Dockerfile vs pin.sh literals | byte-identical (`22.14.0` / `9d942932…f0c2`) |
| 15 | Drift guard RED | scratch-copy Dockerfile `ARG=99.9.9`, run pin.sh with fake docker on PATH | **exit 1**, `error: Dockerfile ARG NODE_VERSION default '99.9.9' drifted from pin.sh literal '22.14.0'`; **docker never invoked** (fail before build); tree untouched |
| 16 | Dockerfile fail-closed order | read `runner/Dockerfile` node layer | `curl` (L83) → `sha256sum -c` (L84) → `tar -xzf` (L86); no `COPY --from=node` |
| 17 | Diff surface | `git diff --name-only base..HEAD` | 74 files, all under `benchmarks/`+`openspec/`+`.gitignore`+`.github/`; **0** `.rs/.py/.toml`; `records/`, `SCHEMA`, `era-1` untouched |

## 2. Evidence I READ and TRUSTED (with supporting corroboration)

- **Real docker build (happy + wrong-SHA)** — task 1.4 line 188 records a genuine `docker build`/`docker run` on this host (docker 29.6.2): happy path prints `v22.14.0`; `--build-arg NODE_SHA256=000…0` fails exit 1 at the `sha256sum -c` gate before extraction. I did NOT re-run docker (expensive; first canonical container run is the human's, bd rc-f4po). Corroborated statically by proof #16 (layer order) and #15 (drift guard). **The measured DIGEST is deliberately NOT recorded yet** — pin.sh records it at the first canonical container run. No overclaim.
- **xmllint-wasm per-tick worker cost (~42ms)** and **saxon per-tick ~1–2ms** — read from task evidence + fixture READMEs; not re-timed. These are honest *caveats*, not performance claims in COVERAGE.
- **11 task evidence blocks** — read in full; each maps to a concrete, reproducible assertion. Spot-checks (#4, #5, #10, #12) confirmed the recorded values byte-for-byte.

## 3. Honest-comparison audit (no hidden handicaps)

- **No cherry-picking:** completeness rule is real and RED-proven (#3). Both contenders implement all 7 active scenarios. `multi-step` correctly excluded (inactive, warning-not-error).
- **fastify framework tax measured fairly:** protocol-B fixtures boot `await app.ready()` before the marker and bind **no** listener (#10) — the module+init cost is paid, matching rust `ctx.start()` / Camel `Main.run`. Only `http-server` binds, per its wire contract.
- **XML engine fairness disclosed:** all 4 XML READMEs name the engine and its JVM counterpart — `saxon-js` (Saxon-JS ≠ Saxon-HE, explicitly stated) vs Saxon; `xmllint-wasm` (libxml2/wasm, "wasm overhead as a documented caveat") vs Xerces-J. The SEF/schema compile sits in the pre-marker init slot, the honest counterpart of the JVM once-per-process compile (#12). Node XML fixtures reuse only the shared byte-pinned assets → digest parity by construction.
- **Zero measurements claimed:** COVERAGE Node axis is 14× `? open-if`, no numbers (#6). First publication is the human's container run (bd rc-f4po).
- **Runtime provenance:** nodejs.org tarball, SHA256-pinned, fail-closed before extraction (#13–#16). No mutable tag, no official-image glibc gamble.

## 4. Incident review (Task 3.1 block destruction / restoration)

A conductor-side STAGE-2 string patch destroyed the Task 3.1 block before the plan bless; an orphan checkbox masked it; a worker caught the symptom (bd rc-rp2d, closed); the block was restored verbatim and the plan RE-BLESSED by e_glm with byte-identical verification. **Judgement: sound.** The current tasks.md Task 3.1 block (lines 404–476) is complete, coherent with the implemented xsd fixtures, and its evidence matches what I executed (#3 exercised the xslt sibling of the same guard; the xsd cells register and resolve in #1). The failure was detected, tracked, and corrected with a re-bless — the evidence trail is intact and the artifact is whole.

## 5. Pre-existing open items (NOT introduced by this change)

- **rc-am22** — `rust-camel-lib` http-server no longer emits per-request lines at HEAD; its smoke label fails id=1 on fresh runs. Node fixtures pass. Pre-existing on main; out of scope.
- **rc-dh7t** — bare auto-discovery dry-run aborts on the inactive `multi-step` dir; the canonical full-suite dry-run therefore requires the explicit 7-scenario list (which is exactly what #1 uses, exit 0). Pre-existing; out of scope. **This is why the literal `bench run --dry-run` bare command is not the acceptance path** — correctly documented in tasks.md 3.3.

## 6. Disclosures the merge commit body MUST carry

1. **Zero measurements published.** All 14 Node cells are `open-if`; first container-hosted run is human-invoked (bd rc-f4po). No req/s, latency, or ranking is claimed by this change.
2. **XML engines differ from the JVM side by construction** — `saxon-js` (≠ Saxon-HE) and `xmllint-wasm` (libxml2/wasm, per-call worker + wasm overhead caveat) vs Saxon / Xerces-J. Cross-engine, not cross-implementation; documented per-fixture. Output byte-parity across runtimes is NOT asserted (serializers differ); INPUT digest parity IS (byte-identical shared assets / canonical payloads).
3. **fastify protocol-B tax is boot-only** — `await app.ready()` with no listener in the 6 non-http-server scenarios; only http-server binds.
4. **Node runtime is a SHA256-pinned nodejs.org tarball** (22.14.0), fail-closed before extraction; the built-image DIGEST is recorded by pin.sh at the first canonical container run, not in this change.
5. **Pre-existing, out-of-scope:** rc-am22 (rust-camel-lib http-server smoke id=1) and rc-dh7t (bare auto-discovery aborts on inactive multi-step). Full-suite dry-run uses the explicit 7-scenario list.
6. **Task 3.1 block was destroyed then restored verbatim and the plan re-blessed** (bd rc-rp2d closed) — recorded for provenance.

## 7. Verdict

**BLESSING GRANTED.**

No overclaim survives inspection: every performance number is withheld, every handicap is disclosed, the completeness rule is enforced and RED-proven, the runtime is hash-pinned fail-closed, and the change touches nothing outside the benchmark zone. The two open items are genuinely pre-existing. The incident was caught and cleanly repaired. Authorize the squash-merge, provided the merge commit body carries the six disclosures in §6.
