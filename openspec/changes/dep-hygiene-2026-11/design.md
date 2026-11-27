# Design: dep-hygiene-2026-11

## Approach

Lock-only dependency update. No source, manifest, or feature edits.

1. Record the baseline (done in design, 2026-11-27 on `main` 1de271e1d):
   `Cargo.lock` 1263 packages; `cargo tree -d` 59 duplicate names (distinct
   `(name, version)` pairs; 5539 dump lines); `parking_lot` 0.12.5 + 0.13.0;
   outdated direct deps: axum-server, bytes, filetime, genai, hyper,
   jsonpath-rust, parking_lot, rdkafka, rustls-pki-types, rustls-webpki, sha2,
   trybuild, ureq, walkdir, webpki-roots, zip.

2. Update exactly the six in-scope packages with
   `cargo update -p <crate> --precise <target>`:
   `bytes@1.12.2`, `sha2@0.11.2`, `trybuild@1.0.125`, `ureq@3.5.2`,
   `walkdir@2.5.1`, `hyper@1.13.0`. `filetime` stays 0.2.29 exactly as on
   `main` (see the out-of-batch record).
   All manifest pins already admit these targets; `ureq-proto 0.6.5 → 0.7.1`
   is a replacement dragged by ureq, not an addition. Cargo re-disambiguates
   duplicate edges when it rewrites the lock (ureq flips
   `serde-saphyr → base64 0.22.1` and `tempfile → getrandom 0.3.4`); both
   requirements are open ranges, so the baseline edges are restored
   (`0.23.1`, `0.4.3`) and `cargo metadata --locked` validates the result.

3. Re-measure: lock diff, duplicate set (exact metric:
   `cargo tree -d --workspace --locked | sed -nE 's/^([A-Za-z0-9_-]+)
   v([^ (]+).*/\1 \2/p' | sort -u | awk '{print $1}' | uniq -d | wc -l`
   — baseline 59; compare the name SETS, not only the count), `cargo audit`,
   `cargo outdated --root-deps-only`. Record sha2-derived fixture-hash
   deltas: module-cache/file hashes in `camel-proto-compiler` and the xtask
   hashing helpers, plus a known-answer SHA-256 over fixed input built
   against 0.11.1 and 0.11.2.

4. Verification matrix (the "tests" of this change are pre-existing suites):
   - `cargo build --workspace` (includes examples and benchmarks members).
   - AGENTS.md clippy registry, including the camel-cli `--no-default-features`
     flavor line.
   - `cargo test --workspace --lib`,
     `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`.
   - Lanes: `camel-component-http` + `camel-core` (hyper),
     `camel-component-sql` + `camel-core` (bytes/sha2),
     `camel-endpoint`, `camel-auth`, `camel-endpoint-macros` (trybuild),
     `xtask` (ureq/walkdir/sha2), `camel-file` (bytes),
     `camel-bridge` + `camel-proto-compiler` (sha2), and the
     `sql_deadline_ordering` sql lane.
   - `bash scripts/gates-local.sh` on the final commit (re-run after any
     later commit); its clippy verdict is known-spurious (bd rc-rpv43), so the
     clippy block is also run and recorded by hand.

5. Stop rule: if any bump requires a source edit, changes a rust-camel
   behaviour requirement, or introduces a parallel version, stop and park with
   the evidence. Do not ship a partial batch.

### Out-of-batch records (dispatcher-approved drops)

- **filetime**: 0.2.30 deprecates `set_file_mtime` on rustc 1.99 (build.rs
  sets `rust_199`), so `-D warnings` fails at three test-only sites
  (camel-core `disk_offload_tests.rs:780`, camel-file
  `atomic_write.rs:483,514`). Lock-only scope wins: filetime stays 0.2.29;
  the `std::fs::File::set_times` migration tracked in bd rc-5jzr7 is the
  prerequisite for the 0.2.30 bump.
- **zip**: 9.8.2 adds mandatory `constant_time_eq 0.6.2` next to blake3's
  `0.4.2` (duplicate names 59 → 60). 9.3.0 is the clean ceiling; 9.4.2+ pull
  0.6.2, so zip needs its own decision later. No zip change in this lock.
- **parking_lot**: dedupe impossible. 16+ transitive consumers pin `^0.12`
  (affinitypool, moka, object_store, prometheus, quick_cache, referencing,
  string_cache, surrealdb-*, surrealmx, tokio, hickory-*, futures-intrusive),
  and only kube-runtime 5.2.0 pins `^0.13.0`. The graph keeps both versions;
  no action.

### Risk notes from upstream changelogs

- hyper 1.13.0: no breaking changes; HTTP/1/2 bug fixes (content-length vs
  transfer-encoding, h2 shutdown races) plus an additive executor getter.
  Behavioural fixes are in areas the camel-component-http suite exercises.
- sha2 0.11.2, bytes 1.12.2, walkdir 2.5.1, ureq 3.5.2, trybuild 1.0.125 are
  patch releases with no API change expected; trybuild's stderr formatting is
  verified by the compile-fail suites.

## Affected crates

No crate source changes. Consumers exercised by verification:
`camel-api`, `camel-core`, `camel-component-http`, `camel-component-grpc`,
`camel-component-sql`, `camel-component-file`, `camel-bridge`, `camel-auth`,
`camel-endpoint`, `camel-endpoint-macros`, `camel-integration-test`,
`camel-file`, `camel-jms`, `camel-mqtt`, `camel-validator`, `camel-xslt`,
`camel-proto-compiler`, `xtask`.

## Architecture boundaries

No rust-camel data-plane or control-plane contract changes: no public types,
no DSL, builder, config key, or component surface changes. The change is
confined to `Cargo.lock` version resolution; the workspace manifest pins are
not edited. Hyper and ureq carry upstream runtime fixes, but no rust-camel
behaviour requirement changes. Verification is exercised through existing
component lanes, so any accidental behaviour change surfaces as a test failure
rather than being masked by a source edit.

## Alternatives considered

- **Bump the workspace manifest pins** to the exact new versions: rejected —
  patch-level pins hide future patch updates and the order forbids manifest
  changes beyond what a bump requires.
- **`cargo update` full sweep** (206 transitive updates): rejected — belongs
  to `bump-version.md`'s release sweep and widens the blast radius beyond the
  approved batch.
- **Keep zip 9.8.2 / force `parking_lot` 0.13 / take filetime 0.2.30 with a
  source edit**: rejected by the dispatcher after measurement; the first two
  violate the no-new-duplicate criterion, the third breaks the lock-only
  scope.
