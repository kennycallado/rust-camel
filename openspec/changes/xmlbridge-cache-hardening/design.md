# Design: xmlbridge-cache-hardening

## Approach

Two independent slices under one change. They share no code path (one is Java inside the
bridge process, one is Rust in the shared binary-resolution path).

### Java — bounded, pin-aware schema cache (rc-mac0j)

`SchemaCache` keeps the `ConcurrentHashMap<String, Schema>` lookup but gains three pieces
of bookkeeping, all mutated under the cache's own monitor:

- an insertion-order queue of unpinned digests,
- a set of pinned digests (maintained by the service),
- a monotonic eviction counter.

**Insert (`getOrCompile` / bounded `put`).** Compiles at most once per digest exactly as
today. After inserting, while `size > maxEntries`, evict the oldest unpinned digest that
is not the entry just inserted; skip pinned digests; stop when at or below the bound. If every
entry is pinned, the cache transiently exceeds the bound — pinned entries are never
evicted, and this is the documented overshoot rule. The eviction policy is FIFO by
insertion time over the unpinned population, which needs no reorder on `get` and leaves
the hot path (`ConcurrentHashMap.get`) untouched.

**Pinning.** `XsdValidatorService.registerSchemaSet` (both the legacy single-blob contract
and the decoded-set contract) pins the digest when it stores `schemaHashById[id] = digest`.
`unregisterSchema` and replacement (`evictIfUnreferenced`) unpin the digest when no public
id still references it — unpinning makes the entry evictable at the next insertion
pressure; it does not evict eagerly. The inline path (`getOrCompileSchema`) inserts
unpinned. An inline insert whose digest equals a pinned digest must not downgrade the pin.
Lock order is `registryLock` → cache monitor, never the inverse, so no deadlock cycle
exists.

**Config.** `xml-bridge.schema-cache.max-entries`, Quarkus `@ConfigProperty` with default
`256`; a non-positive value fails startup (fail-loud, matching the bridge's egress
doctrine). The bound is enforced over total entries; evictions can only remove unpinned
entries.

**Observability.** One log line per eviction (bridge logger, info level) plus an
`evictions()` counter readable by tests.

### Rust — actionable cache-dir error (rc-u68ap)

In `camel_bridge::download::ensure_binary_for_spec`, after the existing early returns
(env-var override, workspace build, verified cache hit) and **before** any network fetch,
probe writability by running `create_dir_all(cache_dir/<version>)` and then creating and
deleting a probe file inside it. **Any** I/O failure of that probe (EACCES from a
root-owned directory, EROFS on a read-only filesystem, ENOSPC, ...) maps to the new
`BridgeError::CacheUnwritable { path, source }` whose message names the path, states the
likely cause (directory owned by another user, e.g. a leftover Docker run, or a read-only
filesystem), and names the remedy (repair ownership or permissions of the directory, or
bypass the cache via the bridge's `CAMEL_<BRIDGE>_BINARY_PATH` env override or the
component's `bridge_cache_dir` config). The `create_dir_all` first step keeps fresh
installs working (the version subdirectory does not exist yet) while still detecting an
unwritable parent. Out of scope, explicitly: an existing-but-unreadable cache directory
that fails the *read* side of hit verification (mode-restricted file reads) — that failure
keeps today's error text.

Placement guarantees:

- a verified cache hit returns before the probe, so a read-only-but-valid cache keeps
  working (successful path unchanged);
- a miss in a writable directory passes the probe and proceeds exactly as before;
- the fix lives in the shared spec-driven resolver, so `xml-bridge`, `jms-bridge`, and
  `cxf-bridge` all benefit.

`BridgeError` (in `process.rs`) gains one additive variant; `camel-xslt`'s existing
`XsltError::Bridge(...)` wrap carries the message to the user unchanged.

## Affected crates

- `bridges/xml` (Java): `SchemaCache`, `XsdValidatorService`, config, tests.
- `crates/services/camel-bridge`: `download.rs` (probe + error mapping), `process.rs`
  (new `BridgeError` variant), tests.
- `crates/components/camel-xslt`: none expected (message passthrough only).

## Architecture boundaries

Data/control-plane split untouched: both changes are lifecycle/cache management inside a
data-plane sidecar and its Services-layer resolver. No DSL, runtime, or language surface
changes. No proto/IDL change (the cache bound and the error text never cross the wire;
the Rust side only renders the error string). Consistent with the fail-loud bridge
doctrine of ADR-0036 (fail-closed guards) and the architecture charter ADR-0045:
component/service-level concerns stay inside their crates.

## Phases

### Phase 1: Bounded XML schema cache (rc-mac0j, Java-only)
- **Goal:** inline and registered schema caching is bounded; registered entries survive
  pressure; evictions observable.
- **Dependencies:** none (bridges/xml only).
- **Externally-visible types/interfaces:** configurable `max-entries`, `evictions()`
  counter on `SchemaCache`; no proto/IDL or public Rust surface.
- **Deliverable:** bridges/xml implementation + regression tests; Gradle suite green.
- **Exit-criteria:** bound-respected test, pinned-survival test, eviction-observability
  test pass; full `bridges/xml` Gradle `test` suite passes.

### Phase 2: Actionable bridge cache-dir error (rc-u68ap, Rust)
- **Goal:** an unwritable cache dir produces the actionable error; writable-path behaviour
  unchanged.
- **Dependencies:** none; ordered after Phase 1 for review granularity only.
- **Externally-visible types/interfaces:** additive `BridgeError::CacheUnwritable` variant.
- **Deliverable:** camel-bridge implementation + offline regression tests.
- **Exit-criteria:** unwritable-dir test asserts the actionable error (path + remedy)
  without network; cache-hit-unchanged test passes; fmt/clippy clean for touched crates.

## Alternatives considered

- **Byte-budget bound:** tighter memory cap but JAXP exposes no reliable compiled-schema
  size; an entry-count bound is deterministic and testable (mission allows either).
- **Silent fallback to a temp cache dir:** hides the misconfiguration, re-downloads on
  every run, and contradicts the fail-loud doctrine; actionable error chosen (mission
  allows "and/or").
- **LRU with access-order tracking:** requires reordering on every `get`, churning a lock
  on the hot path; FIFO-by-insertion over unpinned entries chosen and documented.
- **Separate inline cache map:** two structures complicate pin/evict interplay and size
  accounting; single map plus a pin set chosen.
