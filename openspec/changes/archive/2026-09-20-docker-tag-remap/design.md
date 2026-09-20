# Design: docker-tag-remap

## Context

Landed reality (post flavor split): the docker job (release-matrix.yml)
builds three multi-arch (amd64+arm64) variants, pushes to GHCR + Docker
Hub (`IMAGE_NAME = github.repository` = `kennycallado/rust-camel`):

| variant | suffix today | binary | semantic-tag | Dockerfile stage |
|---|---|---|---|---|
| production | `""` | regular musl (scratch) | `regular` | `FROM scratch AS production` |
| alpine | `-alpine` | slim musl (alpine:3.21) | `slim` | `FROM alpine:3.21 AS alpine` |
| gnu | `-gnu` | full gnu (distroless cc) | `full` | `FROM distroless/cc AS gnu` |

Tags pushed per variant today: `{VERSION}{suffix}`, `latest{suffix}`,
`{semantic-tag}` (multi-arch manifest), plus `-amd64`/`-arm64` arch
intermediates. `VERSION = ${GITHUB_REF#refs/tags/v}` — rc tags keep their
`-rc.N` suffix in VERSION.

Leak (verified via Docker Hub API 2026-09-20): rc rehearsals run
`publish: true` and moved all floating tags. `latest` has been regular
content since the rc.2 rehearsal.

REVISED 2026-09-20 (human ruling, after e_opus blessed a names-only
variant; then a second human ruling removed ALL aliases): the alpine
variant is DISCONTINUED and slim moves to a scratch base. Doctrine: the
base is a function of the toolchain — musl flavors ship on scratch, gnu
ships on distroless. No tag changes meaning: `-slim` is BORN on scratch
(new tag), and BOTH `-alpine` and `-gnu` freeze (never pushed again;
pulls keep working). Zero installed base — no legacy aliases, no
alias-drop window.

## Approach

### 1. Prerelease gate on floating tags

Idiom mirrors the existing crates.io gate (`release.yml`: `!contains(
github.ref_name, '-rc.')`). Define once as a job-level env:

```yaml
env:
  STABLE: ${{ !contains(github.ref_name, '-rc.') }}
```

Gate the FLOATING tag entries only:
- Docker metadata step (579-589): its `tags` output has zero downstream
  consumers (build-push steps hardcode their lists) — DELETE the step
  entirely rather than simplify it.
- Build and push amd64/arm64: `latest-amd64{suffix}` / `latest-arm64{suffix}`
  entries get gated... build-push-action tags cannot be conditional
  per-line; instead compute the arch tag list in a prior step
  (`steps.tags-amd64.outputs.tags`) that emits the latest-arch entries only
  when STABLE. (Arch `latest-*` intermediates are GHCR-only; the
  immutable `{VERSION}-{arch}{suffix}` entries stay ungated — the manifest
  step needs them as inputs.)
- Manifest loop: iterate `{VERSION}{suffix}` always; iterate the
  FLOATING tags (`latest{suffix}`, `{semantic-tag}`) only when STABLE.

### 2. Variant matrix remap

- `gnu` variant: suffix `-gnu` → `-full` (Dockerfile stage `gnu` stays —
  internal name; content unchanged, distroless cc-debian13).
- `alpine` variant → `slim` variant: suffix `-slim`, semantic-tag `slim`
  (already), artifacts already slim musl. Dockerfile: DELETE the alpine
  stage; ADD a `slim` stage identical to `production` (scratch, COPY
  binary + ca-certificates.crt) — the two scratch stages differ only in
  which artifact the matrix downloads into the context.
- `production` variant: unchanged (suffix `""`, scratch, regular).
- The "Prepare build context" step keeps fetching the CA bundle via
  `docker run alpine:3.21 cat ...` (that is how the bundle is OBTAINED,
  independent of the bases we ship).

### 3. Discontinued families (`-alpine`, `-gnu`)

No legacy aliases (zero installed base). Neither family is pushed again;
existing tags freeze at last content — pinned and floating pulls keep
working, nothing 404s. The migration note points users to `-slim` /
`-full` and documents the DIY alpine wrapper (`FROM alpine:3.21` +
`COPY --from=camel:{VERSION}-slim`) for anyone needing a shell.

### 4. Unsuffixed = regular invariant

No new mechanism: the existing flavor smoke (`--version` per variant,
production→regular) already asserts content. Documented as permanent.

### 5. Tag-set assert (drift guard)

In the manifest step, after pushing, print the exact pushed tag list per
variant and grep-assert the expected set. Exact counts (the assert's
numbers must be exact or every stable release fails):
- stable: every variant = 3 — regular (`{VERSION}`, `latest`, `regular`),
  slim (`{VERSION}-slim`, `latest-slim`, `slim`), full (`{VERSION}-full`,
  `latest-full`, `full`)
- rc: every variant = 1 (`{VERSION}{suffix}`)
Fails loudly on tag-logic drift (the assert-22 pattern applied to tags).

### 6. Docs

- distribution-flavors.md: docker table (new names, slim-scratch doctrine,
  `-alpine`/`-gnu` discontinued + DIY alpine wrapper, unsuffixed=regular
  invariant).
- README docker section: pull examples with new names.
- Migration-note snippet (distribution-flavors.md appendix): ready-to-paste
  text for the 0.51.0 release notes.

## Phases

Single delivery phase — one coherent CI+docs change, ~1 task per concern
(Dockerfile+gate+remap+assert in release-matrix.yml; docs; rehearsal).

## Risks / decisions

- Floating tags already leaked (accepted per e_opus ruling; note ships).
- Slim-on-scratch debuts under the NEW `-slim` tag — no tag changes
  meaning; `-alpine` freeze is covered by the migration note.
- Edge field-triage shell is lost on the shipped image (accepted; DIY
  wrapper documented; alpine variant can return later additively if
  demand appears).
- build-push-action cannot condition per-tag-line → arch tag lists computed
  in a shell step (small, readable).
- Multi-registry: gates apply to BOTH registries (all loops already
  dual-tag). Attestation steps unaffected (subject = digest).
- `latest-regular` family deliberately NOT introduced: unsuffixed IS the
  regular family; avoids duplicate meaning.
