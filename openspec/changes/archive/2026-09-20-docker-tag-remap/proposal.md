# Proposal: docker-tag-remap

## Why

Docker tag names must name flavors, not libc. Today the kafka-capable image
is `-gnu` — a libc detail users do not care about — and the slim image is
`-alpine` (also an implementation detail). The 2026-09-17 e_opus verdict
(§5) flagged this asymmetry and prescribed flavor-named tags.

Additionally, a verified pipeline defect: `release.yml` fires with
`publish: true` on every `v*` tag — including `-rc.` rehearsals — and the
docker job has no prerelease gate on floating tags. The v0.51.0-rc.2
rehearsal (2026-09-20) therefore moved `latest`, `latest-alpine`,
`latest-gnu`, and the plain semantic tags `regular`/`slim`/`full` as if it
were a stable release. e_opus ruling (ses_f41091deaffeeE5yJE1SDRSp0A,
2026-09-20): accept the de-facto `latest`→regular flip (it is the desired
end-state, consistent with the binstall regular default), gate the leak
class, and ship the migration note in the 0.51.0 release notes. The strict
one-release pin was reversed: its premises were falsified (the flip already
happened, and the pre-flip `latest` content carried the kafka-silent-reject
bug).

## What Changes

- Prerelease gate: `-rc.` runs MUST NOT move floating tags (`latest*`,
  plain semantic `regular`/`slim`/`full`). Immutable `{VERSION}`-derived
  tags stay ungated so rehearsals smoke the full tag logic.
- Tag remap on the docker job matrix (release-matrix.yml):
  `-gnu` → `-full` (rename, same distroless content); the alpine variant is
  DISCONTINUED — the slim flavor moves to a scratch base (same pattern as
  the production image: static musl binary + CA bundle). Base becomes a
  function of the toolchain: musl flavors ship on scratch, gnu ships on
  distroless.
- No legacy aliases of any kind (zero installed base — the same premise
  the binstall change established): `-alpine` and `-gnu` tag families are
  DISCONTINUED. Existing tags freeze at their last content (pulls keep
  working, nothing 404s); the migration note points users to `-slim` /
  `-full` and documents the 2-line alpine DIY wrapper for anyone needing
  a shell.
- Invariant: the unsuffixed tag family (`{VERSION}`, `latest`) stays
  = regular permanently (asserted by the existing flavor smoke).
- Rehearsal/publish assert: the manifest step verifies the exact expected
  tag set per variant (catches tag-logic drift the way assert-22 catches
  asset drift).
- Docs: distribution-flavors.md docker table + README docker section +
  migration-note snippet for the 0.51.0 release notes.

Excluded: renaming Dockerfile stage targets (internal build detail),
image content changes (landed in the flavor split), the raw-artifact
re-alias work (rc-5t5fo.9).

## Acceptance criteria

- A `-rc.` tag push produces ZERO changes to floating tags in either
  registry (GHCR + Docker Hub); immutable rc-suffixed tags still push.
- A stable release pushes exactly: `{VERSION}`, `latest`, `regular`,
  `{VERSION}-slim`, `latest-slim`, `slim`, `{VERSION}-full`, `latest-full`,
  `full`. No `-alpine` or `-gnu` tags are pushed; the slim image is
  scratch-based.
- The migration note text exists in docs and is ready for 0.51.0 notes.
- No crate code changes; CI-only + docs.

## Risk budget

- Acceptable: rc rehearsals pushing immutable rc tags; discontinued
  `-alpine`/`-gnu` floating tags freezing at last content.
- Out of bounds: changing image content or flavor closures; touching the
  crates.io publish path; manual registry surgery (ruled out by e_opus).

Bd: rc-5t5fo.6
