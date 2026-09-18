# slim-benchmarks binary size — before/after (mission 115)

Build command (both sides):
`cargo build --release --locked -p camel-cli --no-default-features --features <profile>`
(profile: `slim-http` for BEFORE — the only name existing at that commit;
`slim-benchmarks` for AFTER — the renamed marker, forwarded identically by
the one-release alias; the alias resolves the same feature set, proven by
`slim_alias_resolves_identically`).

Host profile: cargo release (thin LTO, strip = true, codegen-units = 1),
RUSTC_WRAPPER= (sccache off), worktree-local target dir.

| side   | commit     | profile feature  | size (bytes) |
|--------|------------|------------------|--------------|
| BEFORE | 37ec0cb6   | slim-http        | 55,712,576   |
| AFTER  | 5c4a7184 (impl 6f146541..fd97159f) | slim-benchmarks | 55,652,352 |

Historical context only (0.48.0 era, superseded by the 37ec0cb6 pair):
BEFORE at da79b310 was 55,685,632 bytes.

## Interpretation — the honest number

Delta this mission: 55,712,576 − 55,652,352 = −60,224 bytes (−0.11%). Near
zero, as predicted — the small shrink is dead-code elimination of
camel-bundles' gated bridge registration paths; the crates themselves stay
linked via camel-cli's own edges. This is the expected result, not a
failure of the change:

- camel-cli depends UNCONDITIONALLY on all eight bridge crates and
  camel-template through its OWN `[dependencies]`
  (crates/camel-cli/Cargo.toml: camel-component-jms/opensearch/redis/sql/ws,
  camel-xslt, camel-xj, camel-template, camel-component-cxf). Those edges
  keep every crate in the slim closure regardless of camel-bundles.
  Proof (cargo tree inversion at base):
  `cargo tree -p camel-cli --no-default-features --features slim-http -i camel-component-sql`
  shows camel-component-sql pulled by BOTH camel-bundles AND camel-cli
  directly.
- What this mission removed is the camel-bundles-SIDE edge: camel-bundles
  `--no-default-features` now drops seven of the eight bridges from its own
  closure (redis survives via camel-config → camel-redis-repo, out-of-zone),
  and camel-template's engine is optional at the source.
- The size win materializes when the camel-cli bridge-forward mission makes
  camel-cli's own eight edges optional (per-bridge features + `full`
  forwards + BootHandle-carrier removal + SLIM_FORBIDDEN_PREFIXES re-add) —
  that mission also renames the transitional cfg keys.

## camel-tree probes (phase-1 exit criterion, recorded for the ledger)

- `cargo tree -p camel-bundles --no-default-features -e no-dev --prefix none --locked | grep -E '<8 bridges> v' | grep -v '(\*)$' | wc -l` → **1**
  (camel-component-redis only, via camel-config → camel-redis-repo).
- Same command + `--features http-static` → **8** (all bridge crates restored).
- `git diff 37ec0cb6 -- Cargo.lock` → empty (zero lockfile drift).
