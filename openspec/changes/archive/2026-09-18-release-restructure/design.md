# Design: release-restructure

## Approach

Three files, one source of truth:

1. `.github/workflows/release-matrix.yml` (NEW) — everything the tag
   pipeline currently does between trigger and publish, exposed as a
   `workflow_call`:
   - `on: workflow_call` with `inputs: { publish: { type: boolean,
     default: false } }`.
   - The 7-leg build matrix (flavor key, targets, runners, use-cross,
     kafka-probe/install-librdkafka/install-musl-tools gates, version
     flavor probe) moves verbatim from `release.yml`.
   - The docker build/publish job: credential-provider install,
     logins, `buildx imagetools create`/push carry
     `if: inputs.publish`; the build step branches on the same input —
     publish mode keeps today's push pipeline, dev mode uses
     `docker buildx build --load` followed by a local `docker run`
     smoke of the loaded image (`--version` flavor assert).
   - The GH-release job (`softprops/action-gh-release` +
     `cargo xtask changelog` notes, `contents: write`) and the
     crates.io publish job (trusted-publishing credential-provider
     install + `scripts/publish-crates.sh`, `id-token: write`,
     `environment: crates-io`) both exist in the current pipeline
     (verified against release.yml @ f1a70e5f) and move behind the
     same `if: inputs.publish`. The GH-release job's `fetch-depth: 0`
     checkout + changelog generation is a publish-only concern and is
     gated with the rest of that job. The docker `Attest*` steps keep
     their `continue-on-error: true` and, being publish side effects
     (`push-to-registry: true`), sit inside the gated publish job.
   - Job-level permissions are declared once here; callers' effective
     permissions are the INTERSECTION with the caller's declaration —
     the tag wrapper must declare the full set (e_opus footgun 1).
2. `.github/workflows/release.yml` (REWRITTEN thin) — tag push
   trigger (`v*`), `permissions: contents: write, packages: write,
   id-token: write`, `secrets: inherit`, single job calling the
   reusable workflow with `publish: true`. NO `concurrency`
   with cancel (e_opus footgun 3: a cancelled `imagetools create`
   leaves half-published manifests).
3. `.github/workflows/release-dev.yml` (NEW) — `push` on tracked
   branches + `workflow_dispatch`, same call with `publish: false`,
   NO `secrets:` block, `concurrency: { group: release-dev,
   cancel-in-progress: true }` (footgun: cancel belongs ONLY here).

The reusable workflow also runs the feature-closure test
(`cargo test -p camel-cli --test feature_profiles`) on the dev path —
the resolver-2 unification tripwire — so every dev run re-proves the
leg closures the matrix builds.

## Affected crates

- none — `.github/workflows/**` only. The workflow shells into
  existing cargo/xtask surfaces; no crate source changes.

## Architecture boundaries

CI is a consumer of the build surface, not a participant in it. The
refactor moves orchestration only: feature composition (camel-cli
markers), probes (kafka-probe, version-flavor), and publish targets
stay byte-equivalent for the tag path. The dev harness intentionally
exercises the SAME composition root the release uses — that symmetry
is the point of the change (no temp-vs-final drift). No Runtime/DSL/
Components/Services/Languages/Functions boundary is crossed.

## Phases

Single-phase: the three files land as one coherent slice. Sequencing
inside the change is fixed by review order (extract → wrap → dev) but
the artifacts are not independently shippable — a half-extracted
matrix would leave the tag pipeline broken.

e_opus final-gate notes carried from bd rc-5t5fo.4 (authoritative):
(1) caller tag-wrapper MUST declare `contents:write, packages:write,
id-token:write` AND `secrets: inherit` — reusable permissions
INTERSECT with caller's; (2) gate the ENTIRE publish job with
`if: inputs.publish` — dev runs must not even install the trustpub
credential provider; (3) `cancel-in-progress` ONLY on the dev wrapper,
NEVER on tag release; (4) run the feature-closure test in the dev
harness (resolver-2 unification tripwire).

## Self-grill record

**Questions generated:**
1. [glossary] Does the new spec domain `release-pipeline` collide with
   any existing `openspec/specs/*` domain, or does an existing domain
   (e.g. `publish-topology`, `publish-registration`,
   `cli-feature-profiles`) already own this normative surface?
2. [sharpen] "Tag semantics byte-equivalent for v-next" — is that
   claim provable/verifiable as stated? What exactly is asserted equal,
   and how would a reviewer check it?
3. [scenario] With `publish: false`, does gating "the entire publish
   job" actually leave a side-effect-free run, given the current
   pipeline has FOUR jobs (build, release, publish, docker) — could a
   step with an external effect slip through the dev path?
4. [cross-ref] The design claims matrix + probes "move verbatim" and
   GH-release/crates.io "if present" — does release.yml @ f1a70e5f
   match those claims, and is anything existing silently dropped or
   anything invented?
5. [scope] Is anything from rc-5t5fo.5/.6/.8/.9 leaking in, or is any
   of .4's own scope deferred?

**Answers (with citations):**
1. [glossary] No collision. Enumerated `openspec/specs/` (114
   domains); no `release-pipeline` directory exists. Adjacent names
   (`publish-topology`, `publish-registration`) govern crate-graph
   publish order/registration, not CI orchestration;
   `cli-feature-profiles` owns the closure-test semantics this change
   consumes, not the workflow. Outcome: confirm.
2. [sharpen] "Byte-equivalent" is too strong literally — the tag
   wrapper is a rewritten file and the reusable job graph may serialize
   differently. The verifiable claim is behavioral equivalence of the
   publish path: same 7 legs, same probes, same artifacts/images/
   release outputs. The spec delta already states this correctly
   (Scenario "tag run publishes as before" → "behaves equivalently to
   the pre-refactor tag pipeline"). The proposal adjective is prose
   imprecision; the normative spec scenario governs and is testable.
   Outcome: refine (flagged to human; spec scenario stands).
3. [scenario] Against release.yml @ f1a70e5f: `build` (no external
   effect — checkout/build/probe/asserts/upload-artifact to the run's
   own store), `release` (GH release + changelog, `contents: write`),
   `publish` (crates.io, `id-token`), `docker` (logins + push +
   imagetools). Under `publish:false` the design gates `release`,
   `publish`, and docker push/login/attest; `build` runs unchanged;
   docker build switches to `--load` + local smoke. Residual:
   `upload-artifact` runs in dev too but writes only to the run's own
   artifact store (no registry, no secret), so "side-effect-free" holds
   in the sense the spec means (no login/push/release/publish).
   Outcome: confirm (design edit makes four-job gating explicit).
4. [cross-ref] release.yml @ f1a70e5f has exactly the 7-leg matrix,
   kafka-probe/install-librdkafka/install-musl-tools gates, "Assert
   version flavor", "Assert jemalloc linked", rename+upload — all
   covered by "moves verbatim". The GH-release and crates.io jobs DO
   exist (not "if present"); that hedge was an unverified conditional,
   now corrected to verified-present and gated. `Attest*` steps
   (`continue-on-error: true`, `push-to-registry: true`) classify as
   publish side effects inside the gated job. Nothing dropped, nothing
   invented. Outcome: refine (evidence gap closed by design edit).
5. [scope] No leakage. Proposal §"Explicitly excluded" + §"Risk
   budget" name .5 (flavor matrix), .6/.9 (artifact rename, docker tag
   remaps), .8 (binstall), .2 (smoke hardening) as out-of-scope; the
   deltas touch none. .4's scope — reusable extraction, tag wrapper,
   dev wrapper, publish gating, dev feature-closure test — is fully
   covered by the three ADDED requirements. Test target verified at
   `crates/camel-cli/tests/feature_profiles.rs`. Outcome: confirm.

**Outcome:** refine — two prose/evidence sharpenings applied to
design.md (four-job publish gating made explicit; "if present" hedge
corrected to verified-present). Proposal adjective "byte-equivalent"
flagged to the human as imprecise vs the governing spec scenario; left
as-is since the normative spec scenario is correct. No scope leak, no
invented behavior, no dropped existing step, spec domain
collision-free, scenarios testable.
**Self-grill mode:** self-grill-proposals skill
