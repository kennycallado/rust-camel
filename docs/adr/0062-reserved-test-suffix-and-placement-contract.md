# ADR-0062: Reserved Test Suffix and Placement Contract

**Date:** 2026-08-22
**Status:** Accepted
**Origin:** OpenSpec change `test-placement-contract` (bd rc-6760)

## Context

`camel test` runs test documents. A test document names route files, declares
inputs, and states expected outputs. It is not a route.

Before this change, no name separated the two file kinds. `camel run`
filtered test documents in the CLI layer. An explicit `*.test.yaml` glob was
honored as a user override. The matched files then parsed as routes and
failed on unknown fields. The rule lived in the wrong crate and the wrong
layer.

Route discovery already had one reserved-behavior gate. A `.json` route file
loads only under a pattern that explicitly targets `.json`
(`DiscoveryError::JsonRequiresExplicitPattern`). Wildcards never load JSON
silently. Test documents needed the same treatment, plus a placement
contract for where they live.

## Decision

### Rule 1: The suffix is reserved and owned by `camel test`

A file name ending in `.test.yaml` or `.test.yml` names a camel test
document. The suffix is reserved. Only `camel test` consumes such files.
Test documents are YAML only. A `.test.json` name is not test-suffixed and
keeps the JSON explicit-pattern gate.

### Rule 2: Discovery enforces the suffix, not the CLI

`camel_dsl::discovery::is_test_document` is the single suffix rule.
Discovery checks it first in the glob-entry loop. The check runs before the
extension gate, before any read, and before interpolation. A reject
therefore never triggers an environment lookup.

- A wildcard pattern skips test documents with no error. The file is never
  read.
- A literal pattern (no `* ? [ ] { }` metacharacters) that names a test
  document fails with `DiscoveryError::ReservedTestSuffix`. The error names
  the file and names `camel test` as the owner.

This is the explicit-gate idiom, the same family as
`JsonRequiresExplicitPattern`. When the operator names a file, ambiguity
fails loudly. When a wildcard merely brushes the file, discovery stays
silent.

`camel run` passes default globs, `Camel.toml` `routes`, and `--routes`
patterns verbatim to discovery. Watch reload inherits the rule through the
same resolver. `camel lint` applies the exported predicate before it
invokes the engine and prints one info line. The lint corpus gate consumes
the same predicate. No consumer keeps a private copy.

### Rule 3: Colocation is the blessed placement

`routes/foo.yaml` with a `routes/foo.test.yaml` sidecar is the blessed
default. The pair stays together in review, in git history, and in the
scaffold.

A separate test directory is first-class. A test document may declare
`routeFilesFromRoot`. Its entries resolve against the project root. The
root is the nearest ancestor directory that holds a `Camel.toml`, found
by walking up from the document. Monorepo
semantics: the nearest ancestor wins, so a per-service `Camel.toml` anchors
that service. This matches cargo workspace discovery. Anchoring never uses
the git or workspace root. A walk that finds no `Camel.toml` fails with
`TestDocError::NoProjectRoot` and names the walked path.

### Rule 4: No in-string sigils

`routeFilesFromRoot` entries that start with a sigil never resolve; the
load fails with a file-not-found error.

- `@/` is frontend idiom. It imports meaning from a foreign ecosystem.
- `$` collides with `${env:}` interpolation.
- `~/` reads as a home directory.
- URI pseudo-schemes muddy the endpoint model, where schemes name
  components.

A file suffix carries none of these collisions.

## Consequences

- The previous explicit `*.test.yaml` glob override is removed. This is an
  accepted breaking change (pre-1.0). A route-shaped file that used the
  reserved suffix must be renamed.
- `expand_patterns_excluding_test_docs` is deleted from camel-cli.
  Discovery owns the rule, so library consumers and watch reload get the
  same behavior.
- Known cost: `camel run --watch` wakes on a test-document save and reloads
  to a no-op. The suffix skip keeps the reload harmless.
- Known cost: a wildcard glob that matches only test documents reports no
  routes. That report is correct.
- `camel lint` on a test document prints one info line and exits 0.

## Alternatives considered

- Keep the CLI-layer filter. Rejected. Discovery is the single choke point
  for route loading. A CLI filter left library consumers and watch reload
  unprotected.
- Honor an explicit `*.test.yaml` glob as an override. Rejected, with the
  breaking change accepted. The override parsed test documents as routes and
  failed on unknown fields. A loud reserved-suffix error beats a
  silent-shape parse failure.
- In-string sigils (`@/`, `$`, `~/`, URI pseudo-schemes). Rejected. Each
  collides with an existing idiom (Rule 4).
- Reserve `.test.json` as well. Not chosen. Test documents are YAML only in
  this change, and a second format would double the parser surface.
