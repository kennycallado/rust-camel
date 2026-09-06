# Tasks: mutation-hardening

## crates/camel-api

### Task 1.1: IPv4-mapped mirror matrix for the ssrf closure

**Files:**
- `crates/camel-api/src/ssrf.rs` (modified — test additions only, inside the existing `#[cfg(test)] mod tests` at line 147)

**Steps:**
1. Add one `#[test]` fn per mirror class below to `mod tests`, each asserting `is_ssrf_blocked_ip` verdicts with `IpAddr::from_str` parsing (follow the parsing style of the neighboring tests):
   - `v4_mapped_private_loopback_linklocal_blocked`: `::ffff:10.0.0.1`, `::ffff:127.0.0.1`, `::ffff:169.254.1.1` → all `true`
   - `v4_mapped_multicast_zero_octet_blocked`: `::ffff:224.0.0.1`, `::ffff:0.1.2.3` → both `true`
   - `v4_mapped_cgnat_corners`: `::ffff:100.63.0.1` → `false`, `::ffff:100.64.0.1` → `true`, `::ffff:100.127.0.1` → `true`, `::ffff:100.128.0.1` → `false`
   - `v4_mapped_benchmark_pair_blocked`: `::ffff:198.18.0.1`, `::ffff:198.19.255.254` → both `true`
   - `v4_mapped_reserved_blocked`: `::ffff:240.0.0.1` → `true`
   - `v4_mapped_public_stay_unblocked`: `::ffff:8.8.8.8`, `::ffff:8.64.0.1`, `::ffff:199.18.0.1`, `::ffff:223.255.255.255` → all `false`
2. Do NOT add NAT64 tests (the two pins already exist at ssrf.rs:332/:340 — `blocks_nat64_embedding_private_ipv4` / `allows_nat64_embedding_public_ipv4`, retain untouched) and do NOT touch any non-test code or existing test.
3. From the worktree root (bd's Dolt DB is location-independent), append the equivalent-mutant annotation to bd rc-tb93: `bd update rc-tb93 --append-notes` with text starting `EQUIVALENT MUTANT (documented at delivery):` followed by the s[7] `>>`→`<<` survivor explanation (zeroes octets[2]; only is_broadcast/is_unspecified read octets[2]; broadcast also caught by the >=240 arm; unspecified indistinguishable via the o0==0 arm) and the final count `21 of 22 killed, 1 equivalent survivor`.

**Tests:** (executable spec — write exactly these; `cargo test -p camel-api --lib ssrf`)
- `v4_mapped_private_loopback_linklocal_blocked`: source with three `::ffff:` addresses parsed → `is_ssrf_blocked_ip` returns `true` for each → kills the `||`→`&&` mutants between the closure's private/loopback/link-local arms and every other false arm
- `v4_mapped_multicast_zero_octet_blocked`: two `::ffff:` addresses → both `true` → kills the remaining one-arm `||`→`&&` adjacencies (multicast, 0/8)
- `v4_mapped_cgnat_corners`: four corners → false/true/true/false → kills both `>=`→`<` and `<=`→`>` window mutations and the `==`→`!=` on the 100 guard
- `v4_mapped_benchmark_pair_blocked`: two addresses → `true` → kills `==`→`!=` on the 198 guard and the `==`→`!=`/`&&`→`||` pair on the 18/19 octet equality
- `v4_mapped_reserved_blocked`: one address → `true` → kills the closure's `>=`→`<` on 240
- `v4_mapped_public_stay_unblocked`: four addresses → all `false` → kills the reverse-direction `==`→`!=` mutants (8.64.0.1 enters the CGNAT arm under `!=100`; 199.18.0.1 under `!=198`) and pins 223.255.255.255 as the public near-miss (239.255.255.255 is multicast — never asserted public anywhere)

**Acceptance:**
- `cargo test -p camel-api --lib ssrf` passes (existing 28 + 6 new)
- `cargo xtask mutants --file crates/camel-api/src/ssrf.rs --json | wc -l` prints exactly 1 — the survivor JSONL goes to **stdout** (one line per `MissedMutant`; wrapper exits 0 even when survivors exist). Do NOT count `target/mutants.out/outcomes.json` — that file holds all outcomes, not survivors. The 1 survivor is the s[7] `>>`→`<<` equivalent mutant in `nat64_embedded_blocked`
- `cargo fmt --check --all` exits 0; `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0
- `git diff -- crates/camel-api/src/ssrf.rs` shows no hunk outside the `#[cfg(test)] mod tests` block (spec Requirement 3)
- bd rc-tb93 notes contain `EQUIVALENT MUTANT (documented at delivery):`

- [x] 1.1

## crates/components/camel-jms

### Task 1.2: Exact-output redaction tests

**Files:**
- `crates/components/camel-jms/src/config.rs` (modified — test additions only, inside the existing `#[cfg(test)] mod tests` that contains `redact_broker_url_masks_userinfo_and_sensitive_query` at line 605)

**Steps:**
1. Add four `#[test]` fns with `assert_eq!` on the complete returned string:
   - `redact_exact_userinfo_mask`: `redact_broker_url("tcp://admin:secretpass@broker.example.com:61616")` equals exactly `"tcp://***@broker.example.com:61616"`
   - `redact_exact_query_join`: `redact_broker_url("tcp://host:61616?password=p&user=u&keepAlive=true")` equals exactly `"tcp://host:61616?password=<redacted>&user=<redacted>&keepAlive=true"`
   - `redact_exact_bare_at_passthrough`: `redact_broker_url("admin@host")` equals exactly `"admin@host"`
   - `redact_exact_failover_param_boundaries`: for input `"failover:(tcp://host:61616)?jms.userName=admin&jms.password=secret&keepAlive=true"`, split the result on `'?'` and `assert_eq!` the full query part against `"jms.userName=<redacted>&jms.password=<redacted>&keepAlive=true"` (delimiter-exact, not loose contains)
2. Retain the existing tests untouched — including the exact clean-URL assertion already at config.rs:635 and the legacy substring test at line 605 (documents intent).
3. Do NOT modify any non-test code.

**Tests:** (executable spec; `cargo test -p camel-component-jms --lib config`)
- `redact_exact_userinfo_mask`: setup input with userinfo → action call → assert full-string equality with the masked form → kills BOTH known survivors (`idx + 3` → `*` and → `-` on `scheme_end` at config.rs:416: shifted boundaries yield `tcp://adm***@…` / `***@broker…`-prefixed strings, none equal the exact expected)
- `redact_exact_query_join`: setup three-param query (sensitive+sensitive+benign) → action call → assert full-string equality including the `&` join and both `<redacted>` replacements → adds forward-looking discrimination on the join/split path (both known survivors die in `redact_exact_userinfo_mask` alone; this test guards the query construction against future mutations)
- `redact_exact_bare_at_passthrough`: setup `admin@host` (no `://`) → action call → assert unchanged full string → pins the `None` authority arm
- `redact_exact_failover_param_boundaries`: setup ActiveMQ failover composite → action call → assert the query segment equals the fully-redacted three-param string → kills index mutations under the failover shape

**Acceptance:**
- `cargo test -p camel-component-jms --lib config` passes (existing + 4 new)
- `cargo xtask mutants --file crates/components/camel-jms/src/config.rs --json | jq -c 'select(.function=="redact_broker_url")' | wc -l` prints exactly 0 — function-scoped per spec Requirement 2 ("no index-arithmetic mutation inside the function survives"). Survivor JSONL is on **stdout**; do NOT count `target/mutants.out/outcomes.json` (all outcomes). The file's 8 pre-existing survivors in unrelated functions (`default_bridge_cache_dir`, `jms_reconnect_default`, `JmsEndpointConfig::from_uri`) are out of scope — tracked in bd rc-isxj0
- `cargo fmt --check --all` exits 0; `cargo clippy -p camel-component-jms --all-targets -- -D warnings` exits 0
- `git diff -- crates/components/camel-jms/src/config.rs` shows no hunk outside the `#[cfg(test)] mod tests` block (spec Requirement 3)

- [x] 1.2
