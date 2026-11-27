# Proposal: dep-hygiene-2026-11

## Why

Six direct dependencies are behind their latest semver-compatible release
(measured 2026-11-27 on `main` 1de271e1d with
`cargo outdated --workspace --root-deps-only`): bytes, sha2, trybuild, ureq,
walkdir, and hyper. This is the low-risk cut the owner approved. The `zip`
bump, the `parking_lot` dedupe, and `filetime 0.2.30` were re-scoped out by
the dispatcher after measurement (see below); the 0.x breaking bumps (genai,
rdkafka) and the ~206 lock-level transitive updates stay out.

## What Changes

Lock-level updates only — no source changes, no manifest feature changes:

| crate | from | to | consumers |
|---|---|---|---|
| bytes | 1.12.1 | 1.12.2 | workspace pin (api, core, most components) |
| sha2 | 0.11.1 | 0.11.2 | workspace pin (auth, bridge, http, jms, mqtt, validator, xslt, proto-compiler, xtask) |
| trybuild | 1.0.122 | 1.0.125 | dev-dep: camel-endpoint, camel-endpoint-macros, camel-auth |
| ureq | 3.5.0 | 3.5.2 | xtask (release/registration observation) |
| walkdir | 2.5.0 | 2.5.1 | xtask |
| hyper | 1.12.0 | 1.13.0 | workspace pin; camel-component-grpc, camel-integration-test; exercised through the camel-http and camel-core lanes |

`cargo update --precise` for each package name; the manifest pins (`"1"`,
`"0.11"`, `"1"`, `"3"`, `"2"`, `"1"`) already allow every target.
hyper 1.13.0's changelog is bug-fix-only plus one additive HTTP/2 executor
getter; it is still treated as the risky bump and gets the
camel-component-http and camel-core lanes.

**Dropped from this change by dispatcher decision:**

- `filetime 0.2.29 → 0.2.30`: on rustc 1.99 the release deprecates
  `set_file_mtime` (its build.rs sets `rust_199`), so `-D warnings` fails at
  three test-only sites (camel-core `disk_offload_tests.rs:780`, camel-file
  `atomic_write.rs:483,514`). A source edit would stop it being lock-only;
  filetime stays 0.2.29 and 0.2.30 waits on the
  `std::fs::File::set_times` migration tracked in bd rc-5jzr7.
- `zip 9.2.2 → 9.8.2`: 9.8.2 adds mandatory `constant_time_eq 0.6.2` next to
  `blake3 1.8.7`'s `0.4.2` (duplicate names 59 → 60). Newest duplicate-free
  zip is 9.3.0; 9.4.2+ all carry 0.6.2. Zip needs its own decision later.
- `parking_lot` dedupe: impossible in this graph. 16+ transitive consumers
  pin `^0.12` (affinitypool, moka, object_store, prometheus, quick_cache,
  referencing, string_cache, surrealdb-*, surrealmx, tokio, hickory-*,
  futures-intrusive), and only kube-runtime 5.2.0 pins `^0.13.0`. Our only
  direct consumer, camel-core, cannot unify the graph. No action taken.

Explicitly excluded: genai 0.6 → 0.7 and rdkafka 0.39 → 0.40 (0.x =
breaking, each needs its own mission with a docker lane); axum-server,
jsonpath-rust, rustls-pki-types, rustls-webpki, webpki-roots (not in this
batch's scope); the ~206 lock-level compatible transitive updates (release
sweep owns them); any API migration in our code — if a bump forces one, the
bump leaves the batch and the mission parks with the reason.

## Acceptance criteria

- All six packages resolve to the target versions in `Cargo.lock`; `filetime`
  stays 0.2.29 exactly as on `main`; no other package version changes beyond
  what `--precise` drags in.
- `cargo tree -d` duplicate set unchanged: 59 distinct names with 2+ versions
  before and after, identical sets (only `sha2`'s version changes
  0.11.1 → 0.11.2; 5539 dump lines; 1263 packages). Verified separately from
  `Cargo.lock` that no name gains a parallel version. `parking_lot` stays at
  two versions.
- `cargo audit --file Cargo.lock` exit 0 with the four existing
  `.cargo/audit.toml` ignores untouched.
- `cargo build --workspace`, the AGENTS.md clippy registry, `cargo test
  --workspace --lib`, `cargo test -p camel-core --test
  hexagonal_architecture_boundaries_test`, and the affected lanes pass:
  camel-component-http and camel-core (hyper), camel-component-sql and
  camel-core (bytes/sha2), camel-endpoint + camel-auth + camel-endpoint-macros
  (trybuild), xtask (ureq/walkdir/sha2), camel-file (bytes), camel-bridge
  and camel-proto-compiler (sha2).
- `bash scripts/gates-local.sh` exits 0 on the final commit; its clippy
  verdict is known-spurious (bd rc-rpv43), so the clippy block is also run and
  recorded by hand.
- Any change in sha2-derived fixture hashes is recorded before/after in the
  park.

## Risk budget

Acceptable: lock-only churn from `--precise` updates; hyper patch-level
behavioural fixes (no breaking changes reported); trybuild formatting
stability (verified by the compile-fail suites). Out of bounds: any source
edit to accommodate a bump, any new duplicate version in the graph, any
change to the audit ignore list, and any manifest feature change.

Affected crates (consumers only, no edits expected): camel-api, camel-core,
camel-component-http, camel-component-grpc, camel-component-sql,
camel-component-file, camel-bridge, camel-auth, camel-endpoint,
camel-endpoint-macros, camel-integration-test, camel-file, camel-jms,
camel-mqtt, camel-validator, camel-xslt, camel-proto-compiler, xtask.
Refs: bd rc-gnzlt (mission), rc-5jzr7 (filetime set_times prerequisite),
rc-rpv43 (gates-local clippy block).
