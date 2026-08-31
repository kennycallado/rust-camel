# Self-grill record — bench-node (spec blessing gate, 2026-08-30)

Mode: self-grill-proposals skill (non-interactive grill-with-docs).
Artifacts grilled at bless-hash sha256:f1c97c28…96650f. Every answer cites
code/docs read during the grill. No speculation.

## Proposal 1 — Contender completeness rule (spec delta Requirement 1)

**Questions:**
1. [glossary] Is "contender family" defined anywhere in the suite vocabulary?
2. [sharpen] What precisely defines "ACTIVE scenario" — disk dirs or harness wiring?
3. [scenario] Does any existing contender violate the rule as written today?
4. [scenario] Does the rule hold at Phase 1 (one scenario landed) with CI green?
5. [cross-ref] Where does the harness derive the active set today?

**Answers:**
1. [glossary] Not defined. `scenarios/README.md:3-8` defines scenario/contender/cell
   only; "family" appears nowhere in suite vocabulary. New term — must be defined
   where the rule lands. → minor finding.
2. [sharpen] Operationally: presence in `SCENARIO_MARKER` (`harness/run.sh:143-160`,
   exactly 7 keys) — the same registry the unknown-scenario error uses
   (`run.sh:1416-1419`). Disk has 8 dirs (`scenarios/` listing): `multi-step` has no
   marker. Spec's "no harness wiring, no contender set" matches, though
   `multi-step/rust-camel-cli/` exists as era-1 leftover — the marker test is the
   unambiguous one; design should pin it.
3. [scenario] YES — the rule as written is suite-wide and today's suite violates it:
   `camel-standalone-yaml` + `camel-quarkus-yaml-native` lack bridge fixtures by
   explicit decision (`SCENARIO_ARTIFACT_SET=bridge`, `run.sh:161-172`, "do NOT
   expand T4a/T4b"; COVERAGE.md:50-54 e_opus round-5). Design implements a
   family-scoped check (design.md:40-44) — spec text overreaches the design.
   → blocking finding 1.
4. [scenario] NO — a global all-7 admission check makes Phase 1 (http-server only)
   fail its own dry-run (node participates → t2-json fixture missing → abort) and
   would break the base-spec CI gate `bench run --scenarios=t2-json,split-aggregate
   --dry-run` (base spec:154-171) mid-change. Selection-scoped enforcement
   (family with ≥1 fixture among SELECTED active scenarios must cover ALL selected)
   preserves the rule where it bites (canonical runs select all 7) and keeps phases
   landable. → blocking finding 2.
5. [cross-ref] `SCENARIO_MARKER` keys (7) + `SCENARIO_M2_PROTOCOL` (run.sh:1831-1852)
   both enumerate exactly the 7 active scenarios; both agree. Enforceable as
   specified once scope is fixed.

**Outcome:** refine — scope rule to completeness-declaring families + selection-scoped
enforcement; pin "active = SCENARIO_MARKER key".

## Proposal 2 — node-native stdlib/lib line + node-fastify (spec Requirement 2)

**Questions:**
1. [glossary] Does "node-native" collide with existing "native" (Quarkus native-image)?
2. [sharpen] What does "same route logic behind the Fastify server layer" mean for
   timer-driven protocol-B scenarios with no request to serve?
3. [scenario] Is the stdlib-vs-libs line defensible against the JVM baseline (Xerces
   in JDK) and does the fairness documentation hold (saxon-js "same engine family")?
4. [cross-ref] Do existing bridge contenders run XML in-process or via the shared
   bridge subprocess — and which shape do node XML fixtures take?

**Answers:**
1. [glossary] "native" is overloaded (native-image vs node-native). Contender names
   are cell keys (`add_cell`, run.sh:1585) and record strings (SCHEMA cells.contender,
   opaque) — `node-native` is unambiguous in context; acceptable, note in README.
2. [sharpen] Undefined — startup-minimal/split-aggregate/t2-*/bridges are protocol B
   (timer-driven; SCENARIO_M2_PROTOCOL run.sh:1836-1851): no HTTP surface. Fastify
   fixture shape (boot app without listen? app.init()? bind?) is unspecified →
   divergent task implementations. → blocking finding 4.
3. [scenario] Line is defensible: JDK ships Xerces/JAXP in-stdlib (JVM fixtures),
   Rust crates supply XML — Node stdlib genuinely has zero XML. But "same engine
   family as the JVM Saxon" overclaims: saxon-js is Saxon-JS (same vendor
   Saxonica, different engine from Saxon-HE); xmllint-wasm is libxml2-on-wasm vs
   JVM Xerces (different engine + wasm overhead). Spec's own auditability scenario
   (delta spec:79-86) says "cross-engine comparison" — honest; design wording
   should match. → minor finding 8.
4. [cross-ref] Existing bridge cells REQUIRE the shared compiled bridge binary +
   wrapper + PID handshake (`resolve_bridge_scenario_cells` run.sh:1240-1258;
   `V3_BRIDGE_PID_FILE` run.sh:1340; PID cleanup tolerates absence run.sh:2004-2011).
   Proposal puts saxon-js/xmllint-wasm IN the node fixture = in-process, bypassing
   the bridge subprocess — but no artifact says node is exempt from the
   bridge-binary/wrapper/PID contract. → blocking finding 3.

**Outcome:** refine — specify fastify non-HTTP shape; state bridge-seam exemption +
shared-file reuse; align engine-family wording.

## Proposal 3 — Pinned Node runtime (spec Requirement 3)

**Questions:**
1. [sharpen] Is the tarball+SHA256 rationale sound vs `COPY --from=node`?
2. [scenario] glibc/GLIBCXX: does the dist tarball actually run on jammy?
3. [cross-ref] "pin.sh verifies them the same way it verifies JVM digests" — does
   that machinery exist?
4. [scenario] Does the stronger verification create inconsistency with maven/gradle?

**Answers:**
1. [sharpen] Sound: base is `eclipse-temurin:21-jdk-jammy` (Dockerfile:21, glibc
   2.35); official node images are bullseye/bookworm (glibc 2.31/2.36) — copying
   node from bookworm onto jammy breaks (symbol versioning). Dist tarball is built
   against old glibc and is the official hash-publishing channel (SHASUMS256.txt).
2. [scenario] Yes — node linux-x64 dist tarballs target ancient glibc floors; runs
   on jammy. "Self-contained" is loose (dynamically linked but portable) — fine at
   design granularity. Whole image is digest-frozen anyway (pin.sh DIGEST), so
   NODE_SHA256 adds build-time fail-closed verification. Sound.
3. [cross-ref] Mis-citation: pin.sh verifies NO JVM digests today — it validates
   image-id FORMAT only (pin.sh:36-54); maven/gradle install unverified
   (Dockerfile:51-71). The NODE_SHA256 `sha256sum -c` check is NEW machinery.
   → minor finding 5. Also pick tarball form (.tar.xz needs xz-utils; .tar.gz
   does not — image has none installed today).
4. [scenario] No inconsistency: strictly stronger verification for a new layer;
   existing layers unchanged.

**Outcome:** refine — fix "same way it verifies JVM digests" wording; pin artifact
form (tar.gz).

## Proposal 4 — Protocol A/B + digest parity contract (spec Requirement 2 scenarios)

**Questions:**
1. [cross-ref] Is the A/B claim true in the harness (http-server=A, rest=B)?
2. [scenario] startup-minimal: is node's process-spawn shape honest vs rust-camel-lib?
3. [cross-ref] Is the digest-parity smoke concrete enough to write tasks from?
4. [scenario] Dry-run without Node: does "node-native resolves, no build" hold for
   all 7 scenarios?

**Answers:**
1. [cross-ref] True: `SCENARIO_M2_PROTOCOL` — http-server "A", the other six "B"
   (run.sh:1836-1851). Claim verified.
2. [scenario] Honest and same shape: ALL contenders are spawned processes
   (`add_cell` argv: `java -jar`, native binaries, wrappers — run.sh:1527-1560);
   rust-camel-lib is a spawned fixture binary, not an in-process call. Node
   `node route.mjs` is the same shape as the JVM. No hidden asymmetry.
3. [cross-ref] Concrete: t2-json canonical body from loadgen builders with golden
   digests (payload.rs:142, golden tests :319-351; fixture-side
   BENCH_INPUT_SHA256 convention pinned in base spec Payload-size axis);
   XML parity is by construction (byte-pinned `shared/bench-payload.xml` +
   identity-transform.xsl, xslt-bridge/shared/); split-aggregate fixed 100-item
   array (README scenario contract). Tasks writable — provided finding 3's
   shared-file reuse is stated for node XML fixtures.
4. [scenario] NO — node-native XML fixtures HAVE package.json (saxon-js/xmllint-wasm)
   → they report would-build, not "resolves from committed script". The dry-run
   scenario text is wrong for 2 of 7. Fix: key behavior on package.json presence.
   → minor finding 7.

**Outcome:** refine — dry-run scenario reworded; parity path confirmed adequate.

## Proposal 5 — Paperwork: COVERAGE, zone contract, README diet, records schema

**Questions:**
1. [cross-ref] Does COVERAGE.md have a shape that can hold "14 rows as open-if"?
2. [cross-ref] Zone contract + README diet: any level-1 additions?
3. [cross-ref] Records schema: any producer change needed for node cells?
4. [scenario] Any measurement claimed anywhere in the artifacts?

**Answers:**
1. [cross-ref] Mismatch: COVERAGE matrix is scenario×metric with contenders only
   parenthetical inside cells (COVERAGE.md:93-106); there are no contender rows to
   add 14 cells to. Representation must be specified (e.g., a "Node contender axis"
   table, 7×2, all `? open-if: first published container-hosted run` per
   scenarios/README.md:49-51 convention). → minor finding 9.
2. [cross-ref] Clean: all edits inside scenarios/, runner/, harness/; level-1
   unchanged (base spec Zone contract:173-188); README diet untouched — technical
   prose goes to scenarios/README (public terminology confinement satisfied, base
   spec:286-300). node_modules must be gitignored (zone allows .gitignore).
3. [cross-ref] None: contender/payload_class strings are opaque (SCHEMA.md:82-90);
   ratio pairing rule (SCHEMA.md:106-111) already handles arbitrary contender sets
   (rust-camel-lib pinned numerator). Records only on human-invoked run (rc-f4po).
4. [scenario] None: all rows open-if; proposal.md:33-34, design phases, spec — zero
   numbers. Consistent with "no human ever types a number" (scenarios/README:71-73).

**Outcome:** refine — specify COVERAGE edit shape; fold stale-count updates
(scenarios/README "six contenders" line 5-8; base-spec t2-json "six artifact
fixtures" :39-43) into scope.

## Verdict

BLESS-WITH-FIXES (4 blocking, 8 minor) — see bless response for the numbered
imperative fix list. No architectural incoherence; all fixes are scoping/wording
edits to proposal.md, design.md, and the delta spec.

---

# Self-grill record — bench-node (PLAN blessing gate, 2026-08-30)

Mode: self-grill-proposals skill. Plan blessed at hash
sha256:64ec684d…3213f39 (supersedes spec blessing). Fresh-eyes pass over
tasks.md (3 phases / 11 tasks) + amended delta + design/proposal, cross-refed
against harness/run.sh, runner/{pin.sh,Dockerfile}, bench facade, ci.yml,
smoke goldens. Every answer cites what was read. No speculation.

**Questions (one per technique, applied across the 6 grill axes):**
1. [glossary/sharpen] Do task references match real code symbols (add_cell signature, both cell-count sites, smoke-log conventions, eip "scenario README")?
2. [sharpen] Is any instruction a pseudo-call a w_fast could paste literally?
3. [scenario] Intermediate states: full-suite dry-run after Phase 1; CI gate between tasks 2.2/2.3; stub-deleted window at 1.4.
4. [cross-ref] Goldens/conventions cited by tests exist (t2-json a0db…, split-aggregate, zone listing, COVERAGE headings, diet regex)?
5. [scenario] Test honesty: any test that cannot fail or asserts unproduced artifacts?
6. [cross-ref] MODIFIED restatement quality vs base spec (sync correctness).

**Key answers (citations):**
1. add_cell = (scenario contender argv marker [reason]) run.sh:1585-1586 — 1.2
   step 3's `"node-native|node-fastify"` literal would create one malformed cell.
   expected_cells has ONE site (run.sh:2873-2886, shared dry-run/measure) —
   1.2 step 4's "BOTH sites" is false. http-server smoke convention is
   `<contender>.log` (smoke/ listing, no port suffix) — 1.3's `-<port>.log`
   deviates from the convention it cites. eip has NO scenario README
   (find t2-realistic-eip -name '*.md' → empty) — 2.4's "README is the source
   of truth" dangles.
2. See 1 — pseudo add_cell needs spelling as two calls.
3. Full-suite dry-run after Phase 1 ABORTS by design (selection-scoped guard:
   node present in selected http-server, missing in 6 selected) — task 1.4
   step 2's "run the full-suite dry-run and confirm http-server shows both
   node cells" is unsatisfiable at that point. CI gate
   (ci.yml:293 --scenarios=t2-json,split-aggregate) goes red on any push
   between tasks 2.2 and 2.3 (node in t2-json only → guard fires inside the
   selection); reverse order mirrors. Stub-deleted window at 1.4 is FINE
   (zero node fixtures in selection = family absent = rule silent; CI-scoped
   dry-run green). Cell arithmetic is fixture-presence-dynamic → counts
   correct on both sides of the phases.
4. t2-json 32768 golden a0db69e1… unanimous across all five contender logs
   (5abe5f00… is the 1024-class log — unambiguous). split-aggregate golden
   123444b4… committed. eip: NO smoke dir, NO golden, fixtures never log
   input digests (route sets own body, main.rs:92) — 2.4's
   eip-input-digest-parity has no target. Zone listing = 7 + .gitignore, but
   `ls -A | sort` emits 8 lines incl .gitignore and locale-dependent order —
   3.3 string equality fails as written. COVERAGE sed-range terminates
   (headings at :121/:136/:154). Diet regex currently 0 hits on
   benchmarks/README.md (test passable). pin.sh flagless, Dockerfile has no
   ARG precedent, no xz-utils, curl present — 1.1 claims all verified.
5. Dishonest/unexecutable: eip-input-digest-parity (no golden, no
   convention); 1.4 full-suite confirmation (outcome impossible); fragile:
   coverage-14-open-if token count breaks if legend says "open-if" (15≠14);
   zone-contract equality (see 4).
6. MODIFIED requirements restate base text verbatim except intended
   six→family-based edits; caveats/rationales retained; split-aggregate
   requirement untouched; sync yields correct canon. One elided noun
   ("eight with the node family" vs "eight cells") — cosmetic.

**Outcome:** BLESS-WITH-FIXES — 2 blocking (1.4 full-suite instruction;
2.4 eip sources/golden), 7 minor. See bless response for numbered
imperatives. No architectural incoherence.
**Self-grill mode:** self-grill-proposals skill
