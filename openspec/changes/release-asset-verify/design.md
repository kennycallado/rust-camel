# Design: release-asset-verify

## Context

The release workflow (`release` job in `.github/workflows/release-matrix.yml`) uploads `dist/camel-*` via `softprops/action-gh-release` and stops there. The action's own eventual-consistency fallback (`Matching asset is present after refresh; continuing...`) can leave an asset un-uploaded while reporting success (bd rc-6ep0i, v0.65.0 run 41810273383). The "Assert artifact flavor suffixes" step already proves the local `dist/` set is complete (22 files: 11 tarballs + 11 sidecars); what is missing is proof that the **release** holds the same set.

## Goals / Non-Goals

**Goals:**
- The release job's exit status reflects the *verified* published asset set.
- Transient 404s during asset listing are absorbed by bounded retry.
- Unverified assets (absent or size-mismatched) get one idempotent repair pass (`gh release upload --clobber` of exactly the unverified set) before the job fails; verified assets are never re-uploaded.
- Failure output names every missing asset explicitly.
- Offline testability via a stubbed `gh`.

**Non-Goals:**
- No change to asset naming, matrix shape, flavor-suffix contract, or packaging.
- No replacement of `softprops/action-gh-release`.
- No checksum re-verification of remote bytes (presence + remote-size equality against the local file; content hashes are not compared, see D3).

## Decisions

### D1: Bash script, `scripts/verify-release-assets.sh`

Consistent with repo convention (flat `scripts/*.sh`). Interface:

```
scripts/verify-release-assets.sh <tag> <dist-dir>
```

- `<tag>` — release tag (`github.ref_name` in the workflow).
- `<dist-dir>` — local directory holding the built assets (`dist`).
- Auth: the workflow step binds `GH_TOKEN: ${{ github.token }}` explicitly (the action's auth does not configure the shell); the `release` job already grants `contents: write`, which `gh release upload` requires. Exits 0 on verified-complete, non-zero otherwise, printing `MISSING: <file>` lines for confirmed-absent (or size-mismatched) assets and an explicit "asset set could not be verified" diagnostic when the listing itself keeps failing.

### D2: Expected set derived from `dist/`

`find "$DIST_DIR" -maxdepth 1 -name 'camel-*' -type f` → basename set. No hardcoded list; the flavor-suffix assert step (upstream) already guards the local set's shape.

### D3: `gh release view --json assets` as the source of truth

Actual release assets read via `gh release view "$TAG" --json assets` (jq-able). Acceptance predicate: each expected file must appear with remote `size` **exactly equal** to the local file's size — this catches truncated and zero-byte uploads, not just presence. An absent or size-mismatched file is "unverified" and eligible for repair; a file present with the matching size is never re-uploaded or deleted. A listing failure (non-zero exit / 404) counts as "attempt failed", not "assets missing", and is retried. Every pass uses a fresh listing — a listing from an earlier pass never authorizes success.

### D4: Retry + repair algorithm

```
attempt 1..VERIFY_MAX_ATTEMPTS (default 6):
    list assets fresh; on success compare against dist set (size equality)
    all present with matching sizes -> exit 0
    listing error (404 etc.) -> sleep VERIFY_SLEEP_SECS (default 10), retry
    some missing/mismatched -> remember set, sleep, retry
after retries:
    if no attempt ever produced an observable listing:
        print "could not be verified" diagnostic, exit 1   # distinct from MISSING
    if still missing/mismatched:
        gh release upload --clobber <unverified files...>   # repair pass
        verify once more (fresh listing, small retry for 404)
        if this final pass cannot obtain a fresh listing either:
            print "could not be verified" diagnostic, exit 1
            # NEVER confirm MISSING from a stale pre-repair observation
        still unverified -> print MISSING lines for each, exit 1
```

Env knobs (overridable for tests): `VERIFY_MAX_ATTEMPTS=6`, `VERIFY_SLEEP_SECS=10`, `VERIFY_REPAIR_MAX_ATTEMPTS=3`, `VERIFY_REPAIR_SLEEP_SECS=10`. Defaults give 6 attempts over ~60s for the initial retry window per the mission order, plus up to ~30s for the post-repair verification window — overall worst case ≈ 90s. `set -euo pipefail`.

### D5: Offline stub tests — `scripts/tests/verify-release-assets-test.sh`

One self-contained bash test script; no bats dependency (repo has none). It:
- creates a temp dir with a fake `dist/` set,
- writes a stub `gh` script onto `PATH` whose behavior is driven by scenario files, recording every `release upload` invocation (file list + flags) for assertions,
- runs the verification script with tiny sleep values,
- asserts exit codes, `MISSING:` output, and the exact upload set per scenario.

Scenarios: success (exit 0, no uploads), missing-asset (exit 1 after repair attempts, `MISSING:` names the file, uploads targeted only the unverified file), 404-then-success (exit 0, no uploads), persistent-404 (exit 1, "could not be verified" diagnostic, distinct from `MISSING:`), successful repair (exit 0, `--clobber` used, verification re-run after upload), failed repair (exit 1, every still-missing file named), repair-then-listing-failure (repair upload happens, all post-repair listings fail → exit 1 with the could-not-verify diagnostic, never a `MISSING:` line from stale data). The 404-then-success stub emits exit 1 with a 404 message on the first `release view`, then a valid asset list.

### D6: Wiring — ci.yml `quality` job + release-matrix.yml

- **ci.yml** (`quality` job — Quality Gates, the normal battery): new step `bash scripts/tests/verify-release-assets-test.sh` — runs on every PR, no Rust needed.
- **release-matrix.yml**: new step `Verify release assets` after `Create release`, running `scripts/verify-release-assets.sh "$GITHUB_REF_NAME" dist`, with a comment block citing bd rc-6ep0i and the v0.65.0 incident so future cleanup does not remove it as redundant.

### D7: Non-zero size check — size equality against local files

Presence alone would miss truncated uploads. The script compares the remote asset `size` to the local file size. Sidecars (`.sha256`) are included — they are part of the published set and are what survived in the incident.

## Risks / Trade-offs

- At most ~90s added to the release job worst case (~60s initial retry window + ~30s post-repair verification) — acceptable for release frequency.
- `--clobber` re-upload of exactly the unverified set (absent or size-mismatched files) cannot damage verified assets (verified assets remain untouched).
- Stub tests depend on `jq` — available on `ubuntu-latest` runners and the release job image; the script degrades to presence-only comparison if `jq` is absent? **No**: `jq` is required and checked up front (`command -v jq`), failing loudly — no silent degradation.

## Migration Plan

Additive only. The verification step runs after upload; if it fails, the release exists but the job is red — which is exactly the desired signal (fail loudly rather than ship incomplete).

## Open Questions

None.
