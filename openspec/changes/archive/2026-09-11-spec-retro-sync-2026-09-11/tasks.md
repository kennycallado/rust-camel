## 1. Verify gaps against main reality

Confirmed on main @ 591e1b85 by inspecting each landing commit and the
current canonical specs (matrix in design.md).

- [x] 1.1 redis-tls sentinel gaps: per-plane CA (7b750a0b), mixed-scheme
  fail-closed (a19d9cb4), live rediss-sentinel:// (854a429c)
- [x] 1.2 http gaps: per-hop fence (83cc7e5f), special-query set
  (164ae82d), ADR-0070 staging (b9deb92d)
- [x] 1.3 itest-batch direct canon edits enumerated (5b55e1c9, c5f40da3,
  da90b768, f37226cb; integration-tier + mock-testkit)
- [x] 1.4 named memory URI convention verified incl. lint message
  (7cc0cb23, rc-gcf9n)

## 2. Author delta specs

- [x] 2.1 redis-tls: MODIFIED standalone CA requirement; ADDED sentinel
  per-plane CA requirement; ADDED sentinel live coverage requirement
- [x] 2.2 http-url-resolution: MODIFIED query composition (special-query
  set + %27 carve-out); MODIFIED host fence (per-hop enforcement)
- [x] 2.3 integration-tier: MODIFIED sqlite mandate (named-URI
  convention); MODIFIED teardown (note: planned → landed)
- [x] 2.4 staged-listener-binding: MODIFIED test-port requirement
  (component-internal listeners, camel-http, readiness poll)

## 3. Validate and review

- [x] 3.1 `openspec validate spec-retro-sync-2026-09-11 --type change`
  passes; canonical specs (incl. the itest-batch-edited ones) re-validated
  coherent
- [x] 3.2 one r_glm review pass over the change artifacts and deltas

## 4. Archive and commit

- [x] 4.1 `openspec archive spec-retro-sync-2026-09-11 --yes` inside the
  worktree (validates, syncs canon, moves the change)
- [x] 4.2 post-archive canon diff spot-check (no old scenario lost)
- [x] 4.3 commit change + archive result as
  `docs(specs): retrospective sync of landed behavior`
