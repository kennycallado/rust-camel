# Tasks: bench-m2-attempted-status

## benchmarks/harness (summarize.py)

### Task 1.1: Evidence classifier, attempted m2 cells, summary rendering

**Files:**
- `benchmarks/harness/summarize.py` (modified)
- `benchmarks/harness/test_summarize.py` (modified)

**Steps:**
1. In `summarize.py`, change `SCHEMA_VERSION = 1` to `SCHEMA_VERSION = 2`
   (line ~92). Leave `INDEX_SCHEMA_VERSION = 1` untouched. Update the
   module docstring's "(run.json v1 …)" phrasing to v2.
2. Add a module-level constant `ATTEMPT_STATUSES = ("unconverged",
   "attempted-timeout")` and a classifier:
   `def classify_m2_attempt(cell_round_dirs: list[Path]) -> Optional[dict]`
   taking every round-directory `Path` that exists for one m2 cell.
   Returns `None` (cell stays absent/MISSING) or
   `{"status": ..., "reason": ...}`:
   - `unconverged` iff some round dir's `protocol-a-summary.txt`
     contains BOTH exact substrings
     `warmup failed-stability: MessageBoundUnconverged` AND
     `status=failed reason=measure-a-error`; `reason` = the exact
     `status=failed reason=...` line.
   - `attempted-timeout` iff some round dir's `exit-codes.txt`
     contains the exact substring
     `# probe reason: no BENCH_LATENCY within 30s timeout`; `reason` =
     that exact line.
   - Both statuses across rounds, or sentinel files matching neither
     rule: emit a loud warning via the existing `_warn` helper naming
     the cell identity and artifact path(s), return `None`.
3. Build an identity→round-dirs map across BOTH walk layouts: the
   nested branch reading `jpath = cdir / "m2-summary.json"` (~line 573)
   AND the flat `<scenario>_<contender>` branch (~lines 549-569).
   Unmeasured identities currently leave no trace — the map must
   record every warm-applicable identity that has at least one round
   dir, in either layout.
4. After the walk, for each warm-applicable m2 identity with zero
   parsed summaries, call `classify_m2_attempt` on its round dirs. On
   a non-None result append an attempted cell:
   `{"scenario": ..., "contender": ..., "metric": "m2", "status": <str>,
   "reason": <nonempty str>, "rounds": <int, count of round dirs
   scanned>}` — NO latency fields. Measured identities (>= 1 valid
   `m2-summary.json`) keep today's shape with NO `status` key; attempt
   evidence ignored.
5. `emit_summary` (~lines 857-925) reads `rows[0]['unit']` and
   `c['median']` for every cell — attempted cells would KeyError and
   kill `main()`/`check_records`. Render attempted cells as a status
   row in summary.md: a line per attempted cell with identity, status,
   reason (e.g. `| http-server/camel-standalone-dsl | m2 | attempted
   (unconverged) | <reason> |`), inserted deterministically (sorted by
   scenario/contender, after that scenario's measured rows). No
   numeric columns for attempted rows. The checksum guard
   (regenerate-from-run.json) must stay byte-identical.

**Tests:** (`python3 -m pytest -q benchmarks/harness/test_summarize.py`)
- `test_m2_unconverged_cell_emits_status`: synthetic run dir,
  `meta.json` roster `http-server,t2-json`, rounds 2; cell
  `http-server/camel-standalone-dsl` has flat round dirs
  `m2-round-{0,1}/http-server_camel-standalone-dsl/protocol-a-summary.txt`
  containing exactly the two real lines
  (`measure-a: error: warmup failed-stability: MessageBoundUnconverged`
  and `status=failed reason=measure-a-error`), no `m2-summary.json`;
  other roster cells measured → summarize → assert cell in run.json
  with `status == "unconverged"`, `reason` containing
  `measure-a-error`, `rounds == 2`, no latency keys,
  `schema_version == 2`; assert summary.md contains a status row for
  the cell and `main()` exit code 0.
- `test_m2_probe_timeout_cell_emits_status`: cell
  `xslt-bridge/rust-camel-cli` with nested round dirs
  `m2-round-{0,1}/xslt-bridge/rust-camel-cli/exit-codes.txt`, first
  line exactly
  `# probe reason: no BENCH_LATENCY within 30s timeout` → summarize →
  assert `status == "attempted-timeout"`, `reason` equals that line,
  no latency keys, exit 0.
- `test_m2_measured_wins_over_evidence`: cell with valid
  `m2-summary.json` in round 1 AND unconverged sentinel in round 0 →
  measured shape (latency keys), NO `status` key. (Regression guard —
  passes before and after; the before-state documents that the walk
  ignores sentinels.)
- `test_m2_conflicting_statuses_stay_missing`: round 0 unconverged
  sentinel, round 1 timeout `exit-codes.txt`, no summary anywhere →
  cell absent from `cells`; warning output names the cell.
- `test_m2_malformed_evidence_stay_missing`: `protocol-a-summary.txt`
  containing only
  `warmup failed-stability: MessageBoundUnconverged` (missing the
  `status=failed` line), no summary → cell absent; warning names cell.
- `test_summary_deterministic_with_attempted_cells`: run summarize
  twice on the unconverged fixture → run.json and summary.md
  byte-identical between invocations.
- `expected`: the four new-behavior tests FAIL before steps 2-5
  (attempted cells absent, schema_version 1); the two guards
  (`measured_wins`, `deterministic`) PASS before and after.

**Acceptance:**
- `python3 -m pytest -q benchmarks/harness/test_summarize.py` exits 0.
- `grep -n 'SCHEMA_VERSION = 2' benchmarks/harness/summarize.py` hits;
  `grep -n 'INDEX_SCHEMA_VERSION = 1' benchmarks/harness/summarize.py`
  still hits.

- [x] 1.1

### Task 2.1: Publish gate, closed shape validation, docs

**Files:**
- `benchmarks/harness/summarize.py` (modified)
- `benchmarks/records/SCHEMA.md` (modified)
- `benchmarks/runner/RUNBOOK.md` (modified)
- `benchmarks/harness/test_publish.py` (modified)

**Steps:**
1. Add `def m2_cell_present(cell: dict) -> bool` — True iff
   (`"status" not in cell` AND `round_values`, `median`, `unit` are
   all present) OR (`cell.get("status") in ATTEMPT_STATUSES` AND
   `cell.get("reason")` is a nonempty str AND none of `round_values`,
   `median`, `unit` present AND `cell.get("rounds")` is an int (bools
   excluded) greater than 0). Everything else — unknown status,
   empty/missing reason, status mixed with latency fields, missing or
   non-positive or boolean `rounds`, bare `{"metric": "m2"}` —
   returns False.
2. In `completeness_gaps` (~lines 1012-1030), an m2 identity is
   present iff (`m2_cell_present(cell)` for its cell in the record)
   OR (identity is listed in the record-level `m2_attempted_cells`
   list — the LEGACY insufficient-samples clause stays; guarded by
   the existing `test_publish.py` test around line 341).
3. Extend publish success output with a split line
   `present m2 <P>/<T>: <M> measured, <A> attempted` (P = measured +
   attempted, T = warm-applicable total). Exit codes unchanged (0
   complete, nonzero + missing list otherwise). `INDEX_SCHEMA_VERSION`
   stays 1; index entries unchanged.
4. Docs move with code: SCHEMA.md — bump "Schema version" to 2,
   document the attempted m2 cell shape (`status`, `reason`,
   `rounds`; no latency fields), rewrite the :128-129 additive-fields
   rule for the v2 boundary (additive WITHIN v2 does not bump;
   v1→v2 is the version bump), keep v1 records readable. RUNBOOK.md
   publish section (~line 199) gains one sentence: attempted cells
   count as present with status, evidence-derived; and item (c)
   (~lines 198-204) drops its stale "`schema_version` 1" and
   "every cell carries `input_sha256`" claims (v2 emitted; attempted
   cells carry no input digest).

**Tests:** (`python3 -m pytest -q benchmarks/harness/test_publish.py`)
- `test_publish_accepts_attempted_cells`: complete record via
  `emit_json`/`emit_summary` where one warm-applicable cell is
  attempted (`status="unconverged"`, nonempty reason, no latency) →
  publish → exit 0, records updated, output contains `attempted`.
- `test_publish_rejects_unknown_status`: attempted cell with
  `status="weird"` → nonzero, missing list names the cell.
- `test_publish_rejects_status_with_latency`: attempted cell also
  carrying `unit`/`median`/`round_values` (any one) → nonzero, listed.
- `test_publish_rejects_empty_reason`: `reason=""` → nonzero, listed.
- `test_publish_rejects_bare_metric_cell`: cell
  `{"scenario": ..., "contender": ..., "metric": "m2"}` (no status, no
  measured fields) → nonzero, listed.
- `test_publish_rejects_attempted_without_valid_rounds`:
  parameterized — attempted cell with `rounds` missing, `0`, `True`,
  and `"2"` (non-int) → nonzero, listed, in all four cases.
- `test_v1_record_still_validates`: complete record with
  `schema_version` 1, all measured → publish/check accepted unchanged.
- `test_mixed_v1_v2_index_rebuild`: one v1 + one v2 record (with an
  attempted cell) in the same records dir → rebuild succeeds,
  `index_schema_version` == 1, both entries present.
- `expected`: the six new rejection/acceptance behavior tests FAIL
  before steps 1-3 (attempted cells block publish; invalid shapes
  pass); the two back-compat tests PASS before and after. Existing
  legacy `m2_attempted_cells` test stays green throughout.

**Acceptance:**
- `python3 -m pytest -q benchmarks/harness/test_publish.py` exits 0.
- `grep -n 'schema_version' benchmarks/records/SCHEMA.md | head -1`
  shows version 2 wording.

- [x] 2.1

### Task 3.1: End-to-end replica of the canonical-run gap families

**Files:**
- `benchmarks/harness/test_publish.py` (modified)

**Steps:**
1. Integration-style test through the REAL summarize→publish chain (no
   hand-built run.json): synthetic run dir whose `meta.json` declares
   scenarios `startup-minimal,xslt-bridge` (bridge roster = 6
   contenders; startup-minimal = cold-only, n/a warm). m1 evidence for
   ALL roster identities (both scenarios, every contender). m2:
   `xslt-bridge` gets 4 measured cells + 1 unconverged cell (nested
   round dirs with `protocol-a-summary.txt`, both exact lines) + 1
   attempted-timeout cell (nested round dirs, `exit-codes.txt` first
   line exact). Summarize the dir, then publish into a temp records
   dir.
2. Assert: publish exit 0; output split line reads exactly
   `present m2 6/6: 4 measured, 2 attempted`; run.json contains both
   attempted cells with correct statuses; index has one entry.
3. Negative twin: same fixture minus the `exit-codes.txt` evidence
   (timeout cell now evidence-less, no summary) → publish exits
   nonzero listing exactly that cell identity.

**Tests:**
- `test_e2e_canonical_gap_families_publish`: fixture above →
  summarize → publish → exit 0; split line
  `present m2 6/6: 4 measured, 2 attempted`; both statuses in
  run.json; one index entry.
- `test_e2e_evidenceless_gap_blocks`: same fixture without the
  `exit-codes.txt` → publish nonzero; missing list names the
  evidence-less cell only.
- `expected`: both FAIL before Tasks 1.1-2.1 land, PASS after.

**Acceptance:**
- `python3 -m pytest -q benchmarks/harness/` exits 0 (full suite).
- No test skips; no network, no docker (synthetic fixtures only).

- [x] 3.1
