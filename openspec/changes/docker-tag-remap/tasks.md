# Tasks: docker-tag-remap

Rehearsal note: the end-to-end rc rehearsal (real `v*-rc.*` tag push,
registry floating-tag diff) is a release-time operational step owned by
the runbook that task 1.4 ships (see "Rehearsal runbook" below); it
cannot run inside the task loop (needs a real tag push + CI minutes).
Task 1.1's exhaustive parameter sweep is its local proxy.

## Phase 1: docker tag remap (gate + slim-scratch + assert + docs)

### Task 1.1: Tag-plan script (single source of truth)

**Files:**
- `scripts/docker-tags.sh` (new)

**Steps:**
1. Create `scripts/docker-tags.sh` as a pure tag-plan generator (no
   side effects, no docker calls). CLI:
   `docker-tags.sh <version> <stable|rc> <suffix> <semantic-tag> [--arch amd64|arm64]`
   — prints the exact tag list for ONE docker variant, one tag per
   line, to stdout. Rules (blessed design §1/§5):
   - default (no `--arch`): immutable `{version}{suffix}` — ALWAYS
     emitted; floating `latest{suffix}` and `{semantic-tag}` — emitted
     ONLY when mode is `stable`.
   - `--arch A`: emits `{version}-A{suffix}` always, and
     `latest-A{suffix}` ONLY when mode is `stable` (arch intermediates
     are GHCR-only push targets; the semantic tag is never arch-tagged).
   - rc versions never emit floating tags of any kind (the quarantine).
2. Add a `--test` mode to the same script: runs the fixture table below
   (plus the full parameter sweep: 3 variants × stable/rc × no-arch/
   amd64/arm64 = 18 combinations, each compared against expected output
   derived from the rules), prints a unified diff on mismatch, exits
   non-zero on any failure, prints `docker-tags: N/N fixtures pass` on
   success.
3. Fixture table (exact, from blessed design §5):
   - `0.51.0 stable "" regular` → `0.51.0`, `latest`, `regular`
   - `0.51.0 stable -slim slim` → `0.51.0-slim`, `latest-slim`, `slim`
   - `0.51.0 stable -full full` → `0.51.0-full`, `latest-full`, `full`
   - `0.51.0-rc.3 rc "" regular` → `0.51.0-rc.3`
   - `0.51.0-rc.3 rc -slim slim` → `0.51.0-rc.3-slim`
   - `0.51.0-rc.3 rc -full full` → `0.51.0-rc.3-full`
   - `0.51.0 stable -slim slim --arch amd64` → `0.51.0-amd64-slim`,
     `latest-amd64-slim`
   - `0.51.0-rc.3 rc -full full --arch arm64` → `0.51.0-rc.3-arm64-full`
4. `chmod +x scripts/docker-tags.sh`.

**Tests:** (executable spec — self-test mode)
- `docker_tags_self_test`: action: run `bash scripts/docker-tags.sh --test`;
  assert: exit 0 and output ends with `N/N fixtures pass` where N ≥ 8
  (fixtures + sweep cases).
- `docker_tags_quarantine`: action: run
  `bash scripts/docker-tags.sh 0.51.0-rc.3 rc -full full`; assert: stdout
  is exactly one line `0.51.0-rc.3-full` (no `latest-full`, no `full`).
- `docker_tags_arch_mode`: action: run
  `bash scripts/docker-tags.sh 0.51.0 stable -slim slim --arch amd64`;
  assert: stdout is exactly two lines `0.51.0-amd64-slim`,
  `latest-amd64-slim` (no semantic `slim` line).

**Acceptance:**
- `bash scripts/docker-tags.sh --test` exits 0 with `N/N fixtures pass`
  (N ≥ 8).
- `bash scripts/docker-tags.sh 0.51.0 stable -slim slim` prints exactly
  3 lines: `0.51.0-slim`, `latest-slim`, `slim`.
- Script contains no docker/buildx/registry invocations (pure generator):
  `grep -cE 'docker |buildx|imagetools' scripts/docker-tags.sh` → 0.

- [x] 1.1

### Task 1.2: Dockerfile — slim scratch stage, alpine stage removed

**Files:**
- `Dockerfile` (modified)

**Steps:**
1. Delete the `FROM alpine:3.21 AS alpine` stage (lines 9-13: apk add,
   COPY binary, ENTRYPOINT).
2. Add a `slim` stage identical in pattern to `production` (blessed
   design §2): `FROM scratch AS slim`, `ARG TARGETARCH`,
   `COPY camel-${TARGETARCH} /usr/local/bin/camel`,
   `COPY ca-certificates.crt /etc/ssl/certs/ca-certificates.crt`,
   `ENTRYPOINT ["camel"]`.
3. Leave `production` and `gnu` stage definitions unchanged EXCEPT the
   allocator comment above the gnu stage (line ~23): update
   `musl variants (production/alpine) ship jemalloc` to say
   `production/slim` (the alpine stage no longer exists).

**Tests:** (executable spec — structural verifications)
- `slim_stage_pattern`: action:
  `sed -n '/FROM scratch AS slim/,/ENTRYPOINT/p' Dockerfile`; assert:
  output contains both COPY lines (binary + ca-certificates.crt) and
  `ENTRYPOINT ["camel"]`, and no `RUN` line (scratch needs no package
  step).
- `alpine_references_gone`: action: `grep -n 'alpine' Dockerfile`;
  assert: 0 hits (stage AND comment).
- `stages_complete`: action: `grep -c '^FROM' Dockerfile`; assert: 3
  (scratch production, scratch slim, distroless gnu).

**Acceptance:**
- All three structural tests above pass verbatim.
- `docker build --target slim` in a context dir containing stub
  `camel-amd64`, `ca-certificates.crt`, and the Dockerfile succeeds
  locally (stub binary need not run — build-layer validation only).

- [x] 1.2

### Task 1.3: release-matrix.yml — gate, remap, script-driven tags, assert

**Files:**
- `.github/workflows/release-matrix.yml` (modified)
- `.github/workflows/ci.yml` (modified)

**Steps:**
1. Job `docker` env block (line ~478): add
   `STABLE: ${{ !contains(github.ref_name, '-rc.') }}` next to the
   existing REGISTRY_* vars.
2. Matrix (lines 456-477): rename variant `alpine` → `slim` with fields
   `suffix: "-slim"`, `target: slim`, artifacts unchanged
   (camel-slim-*), `in-dev: false`, `semantic-tag: slim`; rename variant
   `gnu` → `full` with `suffix: "-full"`, `target: gnu` (stage name
   stays), artifacts unchanged (camel-full-gnu), `in-dev: true`,
   `semantic-tag: full`. `production` row unchanged.
3. Delete the `Docker metadata` step (lines 579-589) entirely — its
   output has no consumers (blessed design §1).
4. Before `Build and push amd64`: add a `Compute amd64 tags` step whose
   `run:` block FIRST derives shell vars (they are NOT otherwise in
   scope): `VERSION="${{ steps.version.outputs.VERSION }}"` and
   `MODE="$([ "${{ env.STABLE }}" = "true" ] && echo stable || echo rc)"`;
   then runs `scripts/docker-tags.sh "$VERSION" "$MODE" "${{ matrix.suffix }}"
   "${{ matrix.semantic-tag }}" --arch amd64`, prefixes each line with
   `${{ env.REGISTRY_GHCR }}/${{ env.IMAGE_NAME }}:` and writes the
   result to `$GITHUB_OUTPUT` using the MULTI-LINE heredoc form whose
   KEY is the output name `tags` (the consumer reads
   `steps.tags-amd64.outputs.tags`; a heredoc key `tags-amd64` would
   write the wrong output name): `echo "tags<<EOF" >> $GITHUB_OUTPUT`,
   then one `registry/name:tag` per line, then `echo "EOF" >>
   $GITHUB_OUTPUT`. Same for arm64 (`tags-arm64`). The build-push steps
   consume `tags: ${{ steps.tags-amd64.outputs.tags }}` /
   `${{ steps.tags-arm64.outputs.tags }}` (dropping their hardcoded
   lists) — floating `latest-arch` entries now appear only on stable
   runs because the script gates them.
5. Manifest step (lines 682-692): the existing local
   `VERSION="${{ steps.version.outputs.VERSION }}"` line stays; add
   `MODE="$([ "${{ env.STABLE }}" = "true" ] && echo stable || echo rc)"`;
   replace the hardcoded TAG for-loop
   with a script-driven loop: for each tag line from
   `scripts/docker-tags.sh "$VERSION" "$MODE" "${{ matrix.suffix }}"
   "${{ matrix.semantic-tag }}"` (NO --arch), run `docker buildx
   imagetools create -t {REGISTRY}/${{ env.IMAGE_NAME }}:{TAG}` (for
   BOTH `${{ env.REGISTRY_GHCR }}` and `${{ env.REGISTRY_DOCKERHUB }}`)
   with sources `${{ env.REGISTRY_GHCR }}/${{ env.IMAGE_NAME }}:${VERSION}-amd64${{ matrix.suffix }}`
   and `${{ env.REGISTRY_GHCR }}/${{ env.IMAGE_NAME }}:${VERSION}-arm64${{ matrix.suffix }}`.
   Append each manifest `{TAG}` pushed to a `PUSHED` accumulator (newline-separated).
6. In the same manifest step, after pushing: tag-set assert built
   INDEPENDENTLY of the script loop (not tautological):
   - construct EXPECTED literally in the workflow from the matrix
     fields, with NO possibly-empty concatenation (a blank line would
     fake a drift): `EXPECTED="${VERSION}${{ matrix.suffix }}"`; then,
     gated on the job env (not a shell var):
     `if [ "${{ env.STABLE }}" = "true" ]; then
     EXPECTED="$EXPECTED"$'\n'"latest${{ matrix.suffix }}"$'\n'"${{ matrix.semantic-tag }}"; fi`.
   - `comm -3 <(sort -u <<< "$EXPECTED") <(sort -u <<< "$PUSHED")` must
     be empty; on mismatch print both lists with headers `expected:` /
     `pushed:` and `exit 1`.
   - Arch intermediates are excluded by construction (PUSHED
     accumulates only manifest-loop tags, never arch tags).
7. Verify (do NOT add a duplicate) the docker job's existing
   `actions/checkout` step (line ~487, pinned SHA, gate `!inputs.
   dev-profile || matrix.in-dev` covering every `docker-tags.sh`
   consumer) is reachable before the compute-tags steps.
8. ci.yml docker-smoke job (lines 213-222): rename the alpine smoke to
   the slim stage — `Build slim image`:
   `docker build --target slim -t camel-smoke:slim context/`;
   `Test slim image` runs `camel-smoke:slim --version`. The production
   and gnu smokes (lines 212, 230) stay unchanged. The alpine:3.21
   tool-image pulls (lines 97, 210) stay unchanged (alpine as CA-bundle
   source, not as a shipped stage).
9. Rename the variant case arms in BOTH case blocks that switch on
   `${{ matrix.variant }}`:
   - flavor-lockstep assert (lines 542-547): `alpine)` → `slim)`,
     `gnu)` → `full)` (EXPECTED values stay `slim`/`full`).
   - dev smoke (lines 611-626): `production) EXPECTED="regular";;
     slim) EXPECTED="slim";; full) EXPECTED="full");`.

**Tests:** (executable spec — structural + logic)
- `stable_env_present`: action:
  `grep -n 'STABLE: .*contains(github.ref_name' release-matrix.yml`;
  assert: exactly 1 hit in the docker job env block.
- `no_alpine_gnu_left`: action:
  `grep -nE '"-alpine"|"-gnu"|AS alpine|alpine\)|gnu\)' release-matrix.yml`;
  assert: 0 hits.
- `metadata_step_gone`: action: `grep -n 'docker/metadata-action'
  release-matrix.yml`; assert: 0 hits.
- `script_is_tag_source`: action:
  `grep -c 'docker-tags.sh' release-matrix.yml`; assert: ≥ 3 (amd64,
  arm64, manifest steps).
- `variant_names_renamed`: action:
  `grep -nE 'variant: (slim|full|production)' release-matrix.yml`;
  assert: exactly 3 hits, no `variant: alpine` / `variant: gnu`.
- `assert_step_independent`: action:
  `grep -n 'comm -3' release-matrix.yml`; assert: 1 hit; and
  `grep -c 'PUSHED' release-matrix.yml` ≥ 2 (accumulator + compare).
- `ci_slim_smoke`: action: `grep -n -- '--target alpine' .github/workflows/ci.yml`;
  assert: 0 hits; and `grep -c -- '--target slim' .github/workflows/ci.yml`
  → 1.
- `logic_via_selftest`: action: `bash scripts/docker-tags.sh --test`;
  assert: `N/N fixtures pass` (the workflow consumes this exact logic).

**Acceptance:**
- All eight structural tests above pass.
- `bash scripts/docker-tags.sh --test` exits 0.
- The FILE-WIDE `grep -c 'actions/checkout' release-matrix.yml` count
  is UNCHANGED from this task's baseline (currently 4 — build/release/
  docker/closure-check jobs; this task adds NO new checkout).

- [x] 1.3

### Task 1.4: Docs — flavor table, doctrine, DIY wrapper, migration note, rehearsal runbook

**Files:**
- `docs/src/operations/distribution-flavors.md` (modified)
- `README.md` (modified)

**Steps:**
1. distribution-flavors.md docker section: replace the current tag
   table with the blessed matrix — regular: `{VERSION}`, `latest`,
   `regular` (scratch); slim: `{VERSION}-slim`, `latest-slim`, `slim`
   (scratch, born 0.51.0); full: `{VERSION}-full`, `latest-full`, `full`
   (distroless cc). Add the doctrine sentence: base is a function of the
   toolchain — musl flavors ship on scratch, gnu ships on distroless.
2. Add a "Discontinued tags" subsection: `-alpine` and `-gnu` are never
   pushed again; existing tags freeze at last content (pulls keep
   working, nothing 404s); users are pointed at `-slim` / `-full`.
3. Add the DIY alpine wrapper snippet:
   `FROM alpine:3.21` +
   `COPY --from=ghcr.io/kennycallado/rust-camel:{VERSION}-slim /usr/local/bin/camel /usr/local/bin/camel`
   with one sentence on when to use it (shell/wget needed on the slim
   flavor).
4. Add the migration-note appendix (ready-to-paste for the 0.51.0
   release notes): latest now tracks regular; -slim/-full are the
   flavor-named families; -alpine/-gnu discontinued (frozen); rc
   rehearsals never move floating tags.
5. Add a "Rehearsal runbook" appendix: the release-time verification
   procedure for the floating-tag quarantine — (a) push `v0.51.0-rc.N`
   tag, (b) after the run, query both registries' tag timestamps (Docker
   Hub API + GHCR) and assert `latest`, `latest-slim`, `latest-full`,
   `regular`, `slim`, `full` are UNCHANGED (pre-rehearsal digests), (c)
   assert the rc-suffixed immutable tags exist. Executed at 0.51.0-rc
   release time; follow-up ticket rc-5t5fo.9 owns the post-0.51.0
   verification pass.
6. README docker/install section: replace the flavor table row and
   examples referencing `-alpine` (README:377 table row, README:386
   `latest-alpine sh` example) with the new families; pull examples in
   `rust-camel:{VERSION}-full` / `rust-camel:latest-full` style.

**Tests:** (executable spec — grep verifications)
- `docs_tag_table_updated`: action:
  `grep -c 'latest-slim\|latest-full' docs/src/operations/distribution-flavors.md`;
  assert: ≥ 2 hits; and
  `grep -n 'discontinued' docs/src/operations/distribution-flavors.md`
  → ≥ 1 hit.
- `docs_diy_wrapper`: action:
  `grep -n 'COPY --from=ghcr.io/kennycallado/rust-camel' docs/src/operations/distribution-flavors.md`;
  assert: ≥ 1 hit.
- `migration_and_runbook_ready`: action:
  `grep -n 'migration' docs/src/operations/distribution-flavors.md` →
  ≥ 1 hit in an appendix heading; and
  `grep -n 'ehearsal runbook' docs/src/operations/distribution-flavors.md`
  → ≥ 1 hit.
- `readme_no_stale_tags`: action: `grep -n -- '-alpine' README.md`;
  assert: 0 hits; and
  `grep -n -- '-full' README.md` → ≥ 1 hit (new pull example present).

**Acceptance:**
- All four grep tests above pass verbatim.
- The migration-note appendix is self-contained (a maintainer can paste
  it into the 0.51.0 release notes without editing).
- The rehearsal runbook names the exact floating tags to diff and the
  registry endpoints to query.

- [x] 1.4
