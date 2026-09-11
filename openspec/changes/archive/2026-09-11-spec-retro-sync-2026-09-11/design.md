## Context

Audit-driven specs-only repair. Main @ 591e1b85. Worktree
`/home/shared/rust-camel-worktrees/retro-sync` (branch `retro-sync`), lease
`openspec-specs.lock`, bd rc-x5sbp tracks the mission. No pre-flight
expert (mission-scoped): every gap was verified against main reality
before authoring, with the landing commit inspected in each case.

### Verified gap matrix

| Gap | Evidence on main | Canon state before |
| --- | --- | --- |
| Sentinel per-plane CA + mixed-scheme fail-closed | 7b750a0b, a19d9cb4 (topology.rs; topology_tests.rs `sentinel_ca_read_gate_covers_tls_sentinel_nodes_with_plaintext_endpoint`, `sentinel_tls_mixed_planes_each_trust_their_own_ca_surface`, `sentinel_ca_with_mixed_scheme_node_list_fails_closed`) | redis-tls req 1 ends "CA trust is standalone-only in this change; the sentinel surface is follow-up bd rc-hbde6" — stale |
| Sentinel TLS live coverage | 854a429c `redis_sentinel_tls_test.rs`: TLS-only master + TLS-only sentinel (`tls-replication yes`), round-trip through the cache repository over `rediss-sentinel://`, plaintext negative controls on both ports | redis-tls covers standalone `rediss://` live coverage only |
| Per-hop allowedUriHosts fence | 83cc7e5f (`send_with_ssrf_safe_redirects` checks the endpoint fence on each hop, fails closed naming the fence; ADR-0071) | http-url-resolution fence requirement covers override resolution only |
| WHATWG special-query rejection set | 164ae82d (`is_legal_query_byte` excludes `'`; test pins `"`, backtick, `<`, `>`, `'` each rejected by byte, `%27` rides) | canon pins only `<` ("forbidden byte in header query errors") |
| ADR-0070 http consumer test staging | b9deb92d (staged pre-bound listener consumed by `get_or_spawn`; readiness poll replaces the 50 ms sleep) | staged-listener-binding enumerates only camel-test and camel-component-wasm suites |
| Named memory URI convention | 7cc0cb23 (lint message steers to `sqlite:file:memdb_{name}?mode=memory&cache=shared` with `provider = "sqlx"` pin; pool-sharing probe maxconn=3; fixtures migrated) | integration-tier mandate lacks the convention; teardown note calls the named URI "planned" |
| itest-batch direct canon edits | 688a0d4a..614a9a0a touched `openspec/specs/integration-tier/spec.md` (+44) and `mock-testkit/spec.md` (+5) in commits 5b55e1c9, c5f40da3, da90b768, f37226cb | content is in canon; traceability gap only |

## Goals / Non-Goals

Goals: make the canon match landed behavior; keep every delta traceable
to a landing commit; pass through validate + one r_glm pass + archive
inside the worktree.

Non-Goals: changing any code; re-litigating landed designs; adding new
capabilities; backfilling deltas for behaviors already correctly
canonized.

## Decisions

1. **MODIFIED, not RENAMED/ADDED, where a requirement exists.** The stale
   sentence in redis-tls req 1 and the "planned" note in the teardown
   requirement are factual errors after the landing commits; a MODIFIED
   rewrite (full requirement incl. all old scenarios, per archive
   collision discipline) is the smallest correct repair.
2. **http stays in http-url-resolution.** The component's URL-policy
   capability already exists (`http-url-resolution`); the mission's
   "add minimal requirements only for fence semantics if none exists"
   clause does not trigger. The per-hop fence extends the existing fence
   requirement; the special-query set extends the existing forbidden-byte
   clause of query composition.
3. **classify_host is cited, not separately canonized.** The shared host
   classifier (bd rc-uwaj, `crates/components/camel-http/src/ssrf.rs`) is
   an implementation detail of SSRF validation; the classifier surface at
   camel-api level is already owned by guard-mutation-hardening
   (`is_ssrf_blocked_ip`). Specifying it again in http-url-resolution
   would duplicate a canon the audit did not flag.
4. **itest-batch direct edits: validate, record, do not re-delta.** The
   edits are already in canon; restating them as ADDED deltas would
   double-apply at archive. This change records the four commits
   (5b55e1c9, c5f40da3, da90b768, f37226cb) here for traceability and
   re-validates the affected specs (see tasks 3.1).
5. **Sentinel live-coverage requirement mirrors the standalone one.**
   Same gating contract (`integration-tests` feature, never `#[ignore]`d,
   ADR-0054), stated for the `rediss-sentinel://` surface with both-hop
   encryption and per-port negative controls, matching 854a429c.
6. **Named-URI convention nuance kept verbatim-accurate.** The bare
   `sqlite::memory:?cache=shared` form remains accepted (old scenario
   "shared cache passes" stays) but with sharing not guaranteed; the
   named form is the convention and the lint steers to it including the
   `provider = "sqlx"` pin — exactly the 7cc0cb23 message. The teardown
   note keeps the original planned-URI shape verbatim; only its
   planned→landed framing changes.
7. **Citation convention.** Each new/modified requirement preamble
   carries a parenthetical "(landed in <sha>, bd <id>)" note. This is a
   deliberate new convention for retrospective changes — the commit
   evidence IS the reason the requirement exists — accepted by review
   (r_glm ses_f70209882ffeaqZn5lMRzAhRzb, finding 3).

## Risks / Trade-offs

- MODIFIED requirements must reproduce every old scenario verbatim;
  omission would silently drop canon at archive. Mitigated by diffing
  the archived canon against the pre-change canon after archive.
- Retrospective deltas describe behavior verified at 591e1b85; if main
  moves underneath before merge, scenarios cite commits, so drift is
  attributable.
- Specs-only: no runtime risk; no quality gates beyond openspec validate
  apply (no Rust changed).
