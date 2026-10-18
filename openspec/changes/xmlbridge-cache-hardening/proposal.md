# Proposal: xmlbridge-cache-hardening

## Why

Two cache-related defects in the bridge layer (bd rc-mac0j, rc-u68ap):

1. **Unbounded inline XML schema cache (rc-mac0j).** `bridges/xml` `SchemaCache` is a plain
   `ConcurrentHashMap`. The inline `validate` path (`XsdValidatorService.validateInternal`
   / `getOrCompileSchema`) stores digest-keyed compiled schemas with no registered-ID
   reference and no eviction bound, so a long-lived process validating many distinct inline
   schemas grows memory without limit. Registered schema-set entries are reference-counted
   (a digest is removed only when no public id maps to it), but that reference counting does
   not bound total size, and inline entries are never referenced at all. Confirmed by
   e_opus (mission 361 task 3.4); already unbounded at baseline `9e1e0b54`.

2. **Root-owned bridge cache dir aborts the run (rc-u68ap).** `~/.cache/rust-camel/xml-bridge`
   owned by root (left by a previous Docker run) makes `camel run` abort with the opaque
   `failed to resolve xml-bridge binary: Permission denied` (surfaced at
   `crates/components/camel-xslt/src/component.rs:51`). The same class affects the other
   bridges' caches (jms, cxf) through the shared `ensure_binary_for_spec` path.

## What Changes

- **bridges/xml (Java, rc-mac0j):** the compiled-schema cache becomes bounded and
  pin-aware. Maximum entries is configurable (default 256). Compiled schemas still
  referenced by a registered schema set are pinned and never evicted; unpinned (inline)
  entries are evicted oldest-inserted-first under pressure. Every eviction is logged and
  counted. Thread-safety and hot-path read behaviour are preserved. No silent unbounded
  fallback.
- **crates/services/camel-bridge (Rust, rc-u68ap):** on a cache miss, binary resolution
  probes cache-dir writability before any network work and fails with a new actionable
  error (`BridgeError::CacheUnwritable`) naming the path, the likely ownership problem, and
  the remedy. Verified cache hits are untouched; writable-directory behaviour is unchanged.
  The fix is shared, so jms-bridge and cxf-bridge caches get the same treatment.
- **Explicitly excluded:** a byte-budget bound (entry-count bound only), a silent fallback
  to a temporary cache directory, changes to schema-set import/include scope, any proto/IDL
  change, and native-image rebuild verification (deferred to CI).

## Acceptance criteria

- Java regression: inserting more distinct inline schemas than the bound keeps the cache
  within the bound (eviction observable); a digest still referenced by a registered schema
  set is not evicted and `validateWith` for that id keeps succeeding.
- Rust regression: a non-writable cache dir produces the actionable error naming path and
  remedy (before any download); a verified cache hit in a writable dir resolves unchanged.
- The xml-bridge Gradle test suite and the camel-bridge Rust tests pass.

## Risk budget

- **Acceptable:** additive `BridgeError` variant; new Quarkus config key with a safe
  default; entry-count bound may be transiently exceeded only by pinned entries.
- **Out of bounds:** cargo in the main checkout; network access in tests; Docker or native
  rebuilds; changing the successful-path resolution order for writable caches.

## Affected crates

- `crates/services/camel-bridge` — binary resolution + error variant (rc-u68ap).
- `bridges/xml` (Java) — `SchemaCache`, `XsdValidatorService`, tests (rc-mac0j).
- `crates/components/camel-xslt` — no code change expected; the existing
  `XsltError::Bridge` wrap carries the clearer message.
