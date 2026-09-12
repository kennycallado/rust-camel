# Tasks: fix-job-multiroute-startup

## camel-cli job command

### Task 1.1: force-start all document routes plus end-to-end multi-route tests

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/commands/job/document.rs` (modified, doc comment only)
- `crates/camel-cli/CONTEXT.md` (modified, one startup-rule sentence)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)

**Steps:**
1. In `run_job` (mod.rs), replace the route startup map
   `let is_target = document::uri_base(def.from_uri()) == target_base; def.with_auto_startup(is_target)`
   with `def.with_auto_startup(true)` for every discovered route. A route
   configured `auto_startup: false` in its route file is also forced on.
2. Rewrite the comment block above that map (the block beginning
   "Route-target safety: nothing auto-starts except the send target") to
   state the
   new rule: all document routes start; side-effect safety comes from the
   fail-closed consumer allowlist at load; the send target stays the sole
   entry point; the missing-target and ambiguous-target checks below stay
   because two consumer routes on one base would round-robin both the
   target send and any `to:` hops.
3. Update the module doc comment at the top of mod.rs (the sentences
   "Route side-effect safety: every discovered route is forced to
   `auto_startup = false` except the send target, which is forced on — a
   job's own trigger must start, other consumer routes must not.") to
   describe all-routes startup with safety from the load gate.
4. Update the doc comment on `target_route_ids` in document.rs (currently
   "both would be auto-started and either could consume the send") to the
   round-robin ambiguity rationale: with all routes started, duplicate
   consumer bases would round-robin the send and any hops, so exactly one
   match stays mandatory.
5. Update `crates/camel-cli/CONTEXT.md` (camel job failure-modes prose):
   the sentence stating the job "forces `auto_startup = false` on every
   route except the send target's consumer route" now states the job
   starts every document route and relies on the load-time consumer
   allowlist, with the send target as the sole entry point.

**Tests:** (write first, verify red, then implement)
- `multiroute_direct_hop_with_autostart_false_helper_completes`:
  setup — tempdir + `write_config` + routes file with three routes:
  (a) `id: "job-target"`, `from: "direct:start"`, steps
  `- to: "direct:enrich"` then
  `- to: "seda:side?waitForTaskToComplete=Always"` (the `Always` option
  makes the seda producer await the side pipeline regardless of exchange
  pattern, so the file write completes before the target route does —
  seda `stop()` aborts forwarders without draining in-flight work, so
  without `Always` the file write is a scheduling race the process exit
  would lose);
  (b) `id: "job-enrich"`, `from: "direct:enrich"`, `auto_startup: false`,
  steps `- set_body: {value: "enriched"}`;
  (c) `id: "job-side"`, `from: "seda:side"`, steps
  `- to: "file:<tempdir>?fileName=side.txt"` (substitute the test
  tempdir's absolute path, matching the camel-file test URI form
  `file:{dir}?fileName=name.txt`). Job doc: mode one-shot, timeout 60s,
  capture-reply true, send to `direct:start` body "ping".
  action — `run_job(dir, "job.job.yaml")`.
  assert — exit 0; report outcome `Completed`; `report["reply"]["body"]`
  equals `"enriched"` (proves the `auto_startup: false` helper route was
  forced on and consumed the hop); after exit, `side.txt` exists in the
  tempdir and contains `enriched`. Before the fix this
  test fails with exit 1 and a "no consumer registered" class error on
  stderr.
- single-route regression: no new test; the existing
  `job_one_shot_test.rs` suite (all 19+ tests) must stay green unchanged —
  behavior and exit codes for single-route documents are identical.

**Acceptance:**
- `cargo test -p camel-cli --test job_one_shot_test multiroute_direct_hop_with_autostart_false_helper_completes` passes.
- `cargo test -p camel-cli --test job_one_shot_test` full suite green.
- `cargo fmt --check` clean; `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.1
