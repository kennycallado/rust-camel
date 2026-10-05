# Proposal: redis-state-tier

## Why

The integration tier asserts data at rest through two state families, SQL
and Surreal, on the waist extracted by `waist-extraction` (ADR-0069
section 14). The camel-cache demo team uses Redis heavily (owner
confirmed 2026-10-02). Redis is the next state family (bd rc-w8s70). It
joins as a thin adapter over the waist: no third pre-waist copy, no
generic `state:` verb, no shared state-family trait.

Redis differs from SQL and Surreal in two ways. It has no query language,
so its assertion subject is a named key. It has no embedded engine, so
the scenario tier cannot spin a hermetic in-process backend as the
Surreal `mem://` tier does. The assert surface is therefore a key plus an
explicit type, projected into the shared matcher tuples, and the
end-to-end battery runs in a dedicated Docker CI job.

## What Changes

- Add a `validate` redis target `{redis: {datasource, key, type, ttl?}}`
  reusing the shared expectation grammar verbatim (`rows` or one count
  bound, `columns`, `unordered`, `deadline`).
- Define the type-aware projection to matcher tuples: `string` `[value]`,
  `hash` `[field, value]`, `list` `[index, value]`, `set` `[member]`,
  `zset` `[member, score]`; `columns` selects and reorders the type
  schema, and integral sorted-set scores normalize to JSON integers while
  values outside `[-2^63, 2^63)` fail closed.
- Define TTL handling: an optional `ttl` node (`atLeast` / `atMost`
  whole-millisecond durations) over `PTTL` through the shared count-bound
  algebra, with checked conversions and defined `-2` / `-1` / `0`
  behavior.
- Define missing, wrong-type, and error behavior: each snapshot is one
  atomic read-only `EVAL` (TYPE + read + PTTL) returning verdict data —
  an initially missing or transiently wrong-typed key recovers by the
  deadline, and a mismatch at expiry is a verdict-class failure;
  unsupported kinds fail closed; driver errors are apparatus-class and
  ADR-0051-sanitized. Diagnostics carry schema column names, row ordinals,
  and the observed TTL milliseconds or missing/persistent status only —
  never a member or field identifier.
- Register Redis in the `DatasourceCatalog`: a `RedisPoolFactory` using
  `redis::Client`'s own URL grammar (ACL username and database index,
  unlike the component endpoint parser) and `RedisBundle::with_catalog`,
  wired by the boot. The target reuses the steering resolver and
  `poll_until`.
- Gate the family behind a `redis` Cargo feature forwarding
  `camel-bundles/redis`; add an `integration-redis` CLI/CI job with a
  provisioned Redis container. The default suite stays Docker-free.
- Document the family: `docs/src/testing/scenario-redis.md`, SUMMARY,
  ADR-0069 section 8, and the `camel-integration-test` CONTEXT terms.

Explicitly excluded: any `redis:` prepare action (routes seed through the
landed `redis:` producer); a generic `state:` verb; a shared state-family
trait; component behavior or URI-grammar changes beyond the pool factory
and bundle catalog hook; new provisioning grammar (`testcontainer` /
`user-provided` stay reserved); any change to landed SQL or Surreal
observable behavior.

## Acceptance criteria

- A scenario document can validate Redis string, hash, list, set, and
  sorted-set state, plus a TTL bound, through `target: {redis: ...}` over
  a named datasource, in both feature configurations.
- The family matches the SQL and Surreal seam point for point: named
  datasource resolution through the boot catalog, shared expectation
  grammar, poll semantics, redaction, and a drop-scoped handle lifecycle.
- Feature-off is a named action-time error; no catalog fails closed; no
  server is contacted without the feature.
- The datasource URL uses the driver's full Redis URL grammar (ACL
  username and `/db` index); unsupported schemes and credentials fail
  closed with the URL redacted.
- `integration-redis` proves the feature stands alone against a
  provisioned Redis container; the default suite is untouched.
- Batteries stay green at baseline: redis lib 584 / battery 608,
  feature_profiles 18, ws-lib, and the SQL and Surreal itest batteries.

## Risk budget

Acceptable: a new Cargo feature; a new optional `redis` dependency; a
pool factory and bundle hook; a Docker-only battery in the dedicated CI
job; documentation. Out of bounds: any change to landed SQL or Surreal
observable behavior; a generic state verb or shared state trait; a
container dependency in the default suite; any unredacted value, member,
field value, or `db_url`.
