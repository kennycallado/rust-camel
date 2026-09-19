# Tasks: release-asset-hygiene

## Release pipeline

### Task 1: Build legs pack tarball and sidecar

**Files:**
- `.github/workflows/release-matrix.yml` (modified)

**Steps:**
1. In the `build` job, locate the `Stage binary` step (currently copies the
   bare binary to `dist/<artifact-name><bin-suffix>`). Replace its `run`
   script with:
   ```bash
   set -euo pipefail
   mkdir -p dist
   # flavor probes already ran on the bare binary above — packaging only
   tar -C "target/${{ matrix.target }}/release" -czf \
     "dist/${{ matrix.artifact-name }}.tar.gz" \
     "camel${{ matrix.bin-suffix }}"
   # portable checksum: macOS runners lack coreutils' sha256sum by
   # default; shasum -a 256 prints the identical "<hex>␣␣<name>" format
   if command -v sha256sum >/dev/null 2>&1; then
     ( cd dist && sha256sum "${{ matrix.artifact-name }}.tar.gz" \
       > "${{ matrix.artifact-name }}.tar.gz.sha256" )
   else
     ( cd dist && shasum -a 256 "${{ matrix.artifact-name }}.tar.gz" \
       > "${{ matrix.artifact-name }}.tar.gz.sha256" )
   fi
   ```
   The tar is created with `-C` so `camel`/`camel.exe` sits at archive
   root (flat layout). The `cd dist` makes the sidecar store the bare
   basename, `sha256sum -c`-clean from any download dir. Windows legs
   run this under git-bash, which ships coreutils (`sha256sum`, `tar`).
2. Change the `Upload artifact` step `with.path` to the repo's bridge
   precedent form (single glob, block scalar — see
   `cxf/jms/xml-bridge-release.yml` upload steps):
   ```yaml
   path: |
     dist/${{ matrix.artifact-name }}.tar.gz*
   ```
   The artifact `name:` stays `${{ matrix.artifact-name }}`.
3. Leave both steps' `if:` conditions untouched — they already carry
   `(!inputs.dev-profile || matrix.in-dev-profile) && matrix.upload != false`,
   which gives dev-leg packaging and compile-guard exclusion for free.
4. Delete nothing else. The jemalloc/probe steps above run on the bare
   binary and are out of scope.

**Tests:**
- name: `local-pack-simulation`
- setup: a scratch dir with a fake `camel` binary (`printf '#!/bin/sh\necho hi' > camel && chmod +x camel`).
- action: run the exact `tar -C ... -czf` + checksum sequence from step 1 against it (the `sha256sum` branch); then exercise the fallback by running the else-branch body verbatim (`shasum -a 256 ... > ...sha256`) — a PATH stub cannot route `command -v` away from a real `sha256sum`, so the body is executed directly instead.
- assert: `tar -tzf dist/fake-name.tar.gz` lists exactly `camel` (flat, no directory prefix); `cat dist/fake-name.tar.gz.sha256` matches regex `^[0-9a-f]{64}  fake-name\.tar\.gz$` (two spaces); `cd dist && sha256sum -c fake-name.tar.gz.sha256` exits 0; the extracted file keeps the exec bit (`tar -xzf ... -C out && test -x out/camel`).
- command: `bash -c '<the sequence>'` in a temp dir (no cargo).
- expected: passes only after step 1's exact commands are used.
- name: `workflow-structural-pack`
- setup: worktree with the modified workflow.
- action: run a python script under `nix-shell -p python3Packages.pyyaml` that parses the workflow YAML, finds the `Stage binary` step of job `build`, asserts its script contains `tar -C`, `sha256sum`, and the `shasum -a 256` fallback branch, finds the `Upload artifact` step and asserts its `with.path` equals the single glob `dist/${{ matrix.artifact-name }}.tar.gz*` (block scalar form), and asserts neither step's `if` contains `inputs.publish`.
- assert: all sub-assertions hold.
- command: as above.
- expected: passes after this task; fails on the pre-change workflow.

**Acceptance:**
- `workflow-structural-pack` passes (pyyaml round-trip, conditions unchanged).
- `local-pack-simulation` passes with flat layout + two-space sidecar on both checksum branches.
- No other step in the `build` job changed (`git diff` scoped to the two steps).

### Task 2: Release job asserts the 22-file topology

**Files:**
- `.github/workflows/release-matrix.yml` (modified)

**Steps:**
1. Replace the body of the `Assert artifact flavor suffixes` step's script
   with this verbatim bash (keeps the existing step name and gating):
   ```bash
   set -euo pipefail
   # 12-entry matrix, 11 uploading: 2 slim + 5 full + 4 regular tarballs
   # + 11 matching sidecars = 22 files. Filename-based only.
   REGULAR_NAMES=(
     camel-x86_64-unknown-linux-gnu.tar.gz
     camel-aarch64-unknown-linux-gnu.tar.gz
     camel-x86_64-unknown-linux-musl.tar.gz
     camel-aarch64-unknown-linux-musl.tar.gz
   )
   FULL_NAMES=(
     camel-full-x86_64-unknown-linux-gnu.tar.gz
     camel-full-aarch64-unknown-linux-gnu.tar.gz
     camel-full-x86_64-apple-darwin.tar.gz
     camel-full-aarch64-apple-darwin.tar.gz
     camel-full-x86_64-pc-windows-msvc.tar.gz
   )
   SLIM_NAMES=(
     camel-slim-x86_64-unknown-linux-musl.tar.gz
     camel-slim-aarch64-unknown-linux-musl.tar.gz
   )
   ok=1; slim=0; full=0; reg=0; side=0
   declare -A seen_tgz
   for f in dist/*; do
     b="$(basename "$f")"
     case "$b" in
       *.tar.gz.sha256)
         side=$((side+1))
         tgz="${b%.sha256}"
         [ -f "dist/$tgz" ] || { echo "FAIL: sidecar without tarball: $b"; ok=0; }
         # format: 64 lowercase hex + exactly two spaces + own basename
         head -1 "$f" | grep -Eq "^[0-9a-f]{64}  ${tgz}$" \
           || { echo "FAIL: sidecar format: $b"; ok=0; }
         ;;
       *.tar.gz)
         if printf '%s\n' "${SLIM_NAMES[@]}" | grep -qxF "$b"; then slim=$((slim+1));
         elif printf '%s\n' "${FULL_NAMES[@]}" | grep -qxF "$b"; then full=$((full+1));
         elif printf '%s\n' "${REGULAR_NAMES[@]}" | grep -qxF "$b"; then reg=$((reg+1));
         else echo "FAIL: unexpected tarball: $b"; ok=0; fi
         [ -z "${seen_tgz[$b]:-}" ] || { echo "FAIL: dup: $b"; ok=0; }
         seen_tgz[$b]=1
         ;;
       *)
         echo "FAIL: unexpected file (raw binaries are retired): $b"; ok=0 ;;
     esac
   done
   [ "$slim" -eq 2 ] || { echo "FAIL: slim=$slim want 2"; ok=0; }
   [ "$full" -eq 5 ] || { echo "FAIL: full=$full want 5"; ok=0; }
   [ "$reg"  -eq 4 ] || { echo "FAIL: regular=$reg want 4"; ok=0; }
   [ "$side" -eq 11 ] || { echo "FAIL: sidecars=$side want 11"; ok=0; }
   [ "$ok" -eq 1 ] || exit 1
   ```
2. The `Create release` step's `files: dist/camel-*` glob already matches
   both tarballs and sidecars — leave it unchanged.

**Tests:**
- name: `local-assert-simulation`
- setup: a temp `dist/` with 11 fake tarballs named per the real matrix (2 slim, 5 full, 4 regular) and 11 valid fake sidecars (`printf '%064d  %s.tar.gz' 0 <name>`).
- action: extract the step's bash script from the workflow via pyyaml and execute it against the temp `dist/` (env-stubbed if needed).
- assert: exits 0. Mutation cases each exit non-zero: (a) one tarball renamed to extensionless `camel-x86_64-unknown-linux-gnu`; (b) a sidecar whose tarball is missing; (c) a sidecar with one space separator; (d) 12 tarballs.
- command: run the extractor script under `nix-shell -p python3Packages.pyyaml` (the script writes the step's bash to a file and executes `bash` on it with `dist/` in place).
- expected: all four mutations fail, clean case passes — only true after the step is updated.
- name: `workflow-structural-release`
- setup: modified workflow.
- action: pyyaml asserts the release job's assert step mentions `.tar.gz.sha256`, `11` for sidecars, and that the `Create release` step still uses `files: dist/camel-*`.
- assert: holds.
- command: as above.
- expected: passes after this task.

**Acceptance:**
- `local-assert-simulation` green on clean + all 4 mutations red.
- `workflow-structural-release` green.

### Task 3: Docker job verifies sidecars before extracting

**Files:**
- `.github/workflows/release-matrix.yml` (modified)

**Steps:**
1. The `Download amd64 binary` / `Download arm64 binary` steps stay as-is
   (same `name: ${{ matrix.amd64-artifact }}` / `arm64-artifact`, same
   `if:` gating): each downloaded workflow artifact now contains
   `<artifact>.tar.gz` + `<artifact>.tar.gz.sha256`.
2. Insert a `Verify and extract amd64` step BETWEEN `Download amd64
   binary` and `Assert amd64 binary flavor`, with the same `if:` as the
   amd64 download and `shell: bash`:
   ```bash
   set -euo pipefail
   cd binary-amd64
   sha256sum -c "${{ matrix.amd64-artifact }}.tar.gz.sha256"
   mkdir -p out && tar -xzf "${{ matrix.amd64-artifact }}.tar.gz" -C out
   ```
   Mirror `Verify and extract arm64` step between `Download arm64
   binary` and its consumer, gated `!inputs.dev-profile`, path
   `binary-arm64`, using `${{ matrix.arm64-artifact }}`. The matrix
   references are deterministic per variant (production →
   `camel-x86_64-unknown-linux-musl`, alpine → `camel-slim-...`, gnu →
   `camel-full-...`); do NOT use `camel-x86_64-*` globs — they match
   only the production variant.
3. Update the two real binary consumers to the deterministic extracted
   path:
   - `Assert amd64 binary flavor` step: replace its
     `BIN="$(find binary-amd64 -type f | head -1)"` discovery with
     `BIN="binary-amd64/out/camel"` (docker targets are all Linux,
     `bin-suffix: ""`). Mirror for the arm64 flavor assert if present.
   - `Prepare build context` step: replace its `find binary-amd64` /
     `find binary-arm64` discovery lines with direct copies from
     `binary-amd64/out/camel` and `binary-arm64/out/camel` into the
     context (keeping the context filenames the Dockerfile's
     `COPY camel-${TARGETARCH}` expects).
   - Do NOT touch the Dockerfile itself — its COPY sources are context
     files, unchanged by this task.
4. Do NOT touch the variant matrix, semantic tags, logins, or the
   `latest` alias logic.

**Tests:**
- name: `local-docker-simulation`
- setup: temp dir `binary-amd64/` with a fake `camel-full-x86_64-unknown-linux-gnu.tar.gz` + valid sidecar (names exercising the gnu variant, NOT the production naming).
- action: run the exact verify+extract bash from step 2 with `matrix.amd64-artifact=camel-full-x86_64-unknown-linux-gnu`; then corrupt the tarball (`printf x >>`), `rm -rf out`, and re-run.
- assert: valid case extracts `out/camel` with exec bit; corrupt case exits non-zero at `sha256sum -c` BEFORE creating `out/`.
- command: `bash -c` the sequence in a temp dir.
- expected: passes only with the step's exact ordering (verify THEN extract) and deterministic names.
- name: `workflow-structural-docker`
- setup: modified workflow.
- action: pyyaml script asserts: (a) the docker job contains steps whose scripts include `sha256sum -c` and `tar -xzf` in that order; (b) NO step in the docker job contains the string `find binary-`; (c) the verify steps reference `${{ matrix.amd64-artifact }}` / `${{ matrix.arm64-artifact }}` rather than `camel-x86_64-*` globs; (d) the flavor-assert and build-context steps reference `out/camel` paths.
- assert: holds.
- command: as above.
- expected: passes after this task; the pre-change workflow fails (a) and (b).

**Acceptance:**
- `local-docker-simulation` green with gnu-variant names (valid extracts, corrupt fails pre-extract).
- `workflow-structural-docker` green including the `find binary-` prohibition.
- `git diff` on the docker job touches only download→extract plumbing + the two consumer steps; Dockerfile untouched.

### Task 4: Distribution docs describe the tarball contract

**Files:**
- `docs/src/operations/distribution-flavors.md` (modified)

**Steps:**
1. Asset table: every `camel-<target>` / `camel-slim-<target>` /
   `camel-full-<target>` artifact name in the "Artifact names" column gains
   the `.tar.gz` suffix; note the matching `.tar.gz.sha256` sidecar per
   asset (one sentence under the table).
2. Breaking-change section: add the raw-name retirement paragraph —
   v0.50.0 shipped extensionless names; from the next release the assets
   are `.tar.gz` + sidecars; one-line migration (`curl -O ...tar.gz &&
   sha256sum -c ...tar.gz.sha256 && tar -xzf`).
3. Add the Windows extraction floor note: pre-1803 Windows / Server 2016
   users must supply their own tar (7-Zip/bsdtar).
4. `nix shell nixpkgs#mdbook -c mdbook build docs` must succeed.

**Tests:**
- name: `doc-contract-consistency`
- setup: built docs.
- action: grep the rendered `book/operations/distribution-flavors.html`.
- assert: contains `.tar.gz.sha256`, the retirement paragraph mention of `v0.50.0`, and `7-Zip`; contains no artifact-name cell still showing an extensionless `camel-x86_64-unknown-linux-gnu`-style name in the assets context.
- command: `nix shell nixpkgs#mdbook -c mdbook build docs && grep ...`.
- expected: passes after this task.

**Acceptance:**
- `doc-contract-consistency` green.
- `mdbook build` exits 0.
- English prose, STE-friendly (no new idioms like "under the hood").
