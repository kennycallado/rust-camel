# Tasks: heartbeat

All fleet files live in the MAIN checkout (`.opencode/fleet/` is
gitignored — owner order 235); systemd units live in the user instance.
The only repo-tracked artifact set is `openspec/changes/heartbeat/`.
Workers run shell tests with PATH-stubbed commands for hermetic runs.

Shared conventions for Tasks 1.1 and 1.2 (NEW symbols — keep names exact):

- `FLEET_DIR` env var, default `/home/kenny/dev/rust-camel/.opencode/fleet`.
  ALL file state derives from it: `quota.log`, `thresholds.json`,
  `conductor.json`, `server-auth`, `inbox/`, `health.log`, `buzzer`,
  `watchdog.last-anomaly`, `events.log`, `quota-snap.sh`.
- Scripts call `systemctl`, `journalctl`, `df`, `curl`, `notify-send` by
  plain name (never absolute) so tests can PATH-stub them.
- `set -u` only (no `set -e`): every subcommand failure is handled
  explicitly; the battery never aborts mid-run.
- Times: silence ages via `journalctl -o json` `__REALTIME_TIMESTAMP`
  (microseconds) with `systemctl show -p ActiveEnterTimestamp --value`
  fallback for units with zero journal entries.

## Fleet scripts (main checkout)

### Task 1.1: heartbeat.sh + thresholds.json — hourly battery

**Files:**
- `/home/kenny/dev/rust-camel/.opencode/fleet/thresholds.json` (new)
- `/home/kenny/dev/rust-camel/.opencode/fleet/heartbeat.sh` (new, chmod +x)

**Steps:**
1. Write `thresholds.json` exactly:
   `{"glm_5h_pct": 75, "ocgo_mo_pct": 85, "anthropic_7d_pct": 75}`.
2. Write `heartbeat.sh` (bash, `set -u`) implementing, in order:
   a. If `"$FLEET_DIR"/quota-snap.sh` is executable, run
      `bash "$FLEET_DIR/quota-snap.sh"` (refresh; hermetic test dirs
      simply do not provide the file and use a synthetic quota.log).
   b. Quota check: `LAST="$(tail -1 "$FLEET_DIR/quota.log" 2>/dev/null)"`.
      Normalize Python literals to JSON BEFORE any jq call (the writer,
      quota-snap.sh, emits `None`/`NaN`):
      `LAST_JSON="$(printf '%s' "$LAST" | sed 's/:None/:null/g; s/:NaN/:null/g')"`.
      If `thresholds.json` is missing or not valid JSON (`jq -e .` fails),
      emit digest line `WARN config: thresholds.json missing` (exact spec
      string; this line REPLACES the quota line — no comparison runs) and
      count a WARN — do not run `jq` on it again. If `LAST` is empty, emit
      `quota no-data WARN`. Otherwise read each of `glm_5h_pct`,
      `ocgo_mo_pct`, `anthropic_7d_pct` from `LAST_JSON`; a `null`/empty
      value marks that field stale. Digest line exactly:
      `quota glm_5h=<v|null>/75 anthropic_7d=<v|null>/75 ocgo_mo=<v|null>/85 OK`
      or `... WARN (<breached-or-stale field names with values>)` when any
      field is above its threshold or stale.
   c. Units check: list running units
      `systemctl --user list-units 'fleet-*' --state=running --no-legend`
      (name only per line). A unit is mute when its last journal entry
      (or, with no entries, its `ActiveEnterTimestamp`) is older than
      1800 s AND it has been active longer than 1800 s. Digest line:
      `units running=<n> mute=<n> OK` or
      `units running=<n> mute=<n> WARN (<unit names with silence age in minutes>)`.
   d. Inbox check (only if `"$FLEET_DIR/inbox"` is a directory):
      `find "$FLEET_DIR/inbox" -maxdepth 1 -type f \( -name '*-parked.*' -o -name '*-landed.*' \) -mmin +360`.
      Digest line: `inbox stale=<n> OK` or `inbox stale=<n> WARN (<names>)`.
   e. Disk check: `df -P /home/shared`, percent from column 5. Digest
      line: `disk /home/shared=<pct>%/80% OK` or `... WARN`.
   f. Compose message: first line `[HEARTBEAT HH:MM]` (local time,
      `date +%H:%M`), then one line per emitted check line. Print the
      full digest to stdout.
   g. POST the digest to the master session using the exact selfwake.sh
      gate and shape: read
      `PASS="$(cat "$FLEET_DIR/server-auth" 2>/dev/null)"` and
      `MASTER="$(jq -r .conductor "$FLEET_DIR/conductor.json" 2>/dev/null)"`;
      skip (log `"ev":"skip","reason":"no-pass-or-master"`) if either is
      empty OR `$MASTER` is the literal string `null` — the verbatim
      selfwake.sh guard. Then:
      `curl -s -o /dev/null -w '%{http_code}' --max-time 25 -X POST
      "http://localhost:8080/session/$MASTER/message" -u "opencode:$PASS"
      -H 'Content-Type: application/json' -d "$(jq -n --arg t "$TEXT"
      '{parts:[{type:"text",text:$t}]}')"`.
   h. Append to `"$FLEET_DIR/events.log"`:
      `{"src":"heartbeat","ev":"post","http":<code or null>,"warns":<count>}`
      (or `"ev":"skip","reason":"no-pass-or-master"` when the POST was
      skipped). Exit 0 when zero WARN lines, exit 1 otherwise — the exit
      code follows the WARN count, never the POST result.

**Tests:** (hermetic — build a temp dir `T` with `inbox/`, synthetic
`quota.log`, `thresholds.json`; a temp `bin/` with stub `systemctl`
(echoes one `fleet-fake.service ... running` legend line for
`--state=running`; for `--state=failed` echoes the line
`fleet-dead.service loaded failed failed` only when env `FAILED_LINE=1`,
else nothing; for `show -p ActiveEnterTimestamp --value` echoes
`date -d '2 hours ago' '+%Y-%m-%d %H:%M:%S'`), stub
`journalctl` (for `-n 1 -o json` echoes
`{"__REALTIME_TIMESTAMP":"<now minus $JOURNAL_AGE_SEC seconds, in
microseconds>"}` — the stub reads env `JOURNAL_AGE_SEC`, default `60`;
only the mute test overrides it), stub `df` (echoes
`... <pct>% /home/shared` from env `DF_PCT`, default `48`), stub
`curl` (echoes `200`, or `$CURL_CODE` when env `CURL_CODE` is set);
run `PATH="$T/bin:$PATH" FLEET_DIR="$T" bash heartbeat.sh`)
- `heartbeat-syntax`: file exists and is executable → `bash -n
  /home/kenny/dev/rust-camel/.opencode/fleet/heartbeat.sh` → exit 0.
- `heartbeat-green-digest`: synthetic quota line
  `{"t":"...","glm_5h_pct":40,"anthropic_7d_pct":30,"ocgo_mo_pct":50}`
  with default (fresh) journal timestamp → stdout starts with `[HEARTBEAT `
  and contains exactly 4 check lines, each ending `OK`; `warns` in
  `events.log` is 0; exit 0.
- `heartbeat-threshold-breach`: synthetic quota `glm_5h_pct":90` → the
  quota line ends `WARN` and contains `glm_5h=90/75`; exit 1.
- `heartbeat-python-literal-quota`: seed `quota.log` with a verbatim
  real-world line containing `"anthropic_reset":None` and
  `"ocgo_mo_pct":95` → quota line ends `WARN` and contains
  `ocgo_mo=95/85` (a real breach), NOT `ocgo_mo=null/85`; exit 1.
- `heartbeat-null-field-stale`: synthetic quota `glm_5h_pct":null` →
  quota line ends `WARN` and contains `glm_5h=null/75`; exit 1.
- `heartbeat-config-missing`: `thresholds.json` absent from `$T` →
  stdout contains `WARN config: thresholds.json missing` and NO `quota`
  check line; exit 1; no jq crash (stderr has no `jq: error`).
- `heartbeat-empty-quotalog`: `quota.log` absent from `$T` → stdout
  contains `quota no-data WARN`; exit 1.
- `heartbeat-mute-unit`: `JOURNAL_AGE_SEC=2700` (45-min-old last output,
  unit active for 2 h per the stub) → units line ends `WARN` and contains
  `fleet-fake.service` with a minute age; exit 1.
- `heartbeat-no-post-when-hermetic`: green run with NO `server-auth` or
  `conductor.json` in `$T` → `$T/events.log` last line contains
  `"ev":"skip","reason":"no-pass-or-master"`; no buzzer writes.
- `heartbeat-post-failure-exit`: green data plus stub `server-auth`
  (`testpass`) and `conductor.json` (`{"conductor":"ses_test"}`) in `$T`,
  `CURL_CODE=500` → exit is still 0 (exit follows WARN count, not POST
  success) and the `events.log` line contains `"http":500`.**Acceptance:**
- All 10 tests pass with the exact stub harness described above.
- `bash -n` clean; file mode 0755.
- Real-path dry inspection: running with default `FLEET_DIR` and real
  `server-auth`/`conductor.json` would POST (verified only in Task 2.2 —
  do NOT run for real here).

- [x] 1.1

### Task 1.2: watchdog.sh — thin 5-minute anomaly monitor

**Files:**
- `/home/kenny/dev/rust-camel/.opencode/fleet/watchdog.sh` (new, chmod +x)

**Steps:**
1. Write `watchdog.sh` (bash, `set -u`) implementing, in order:
   a. Quota: read `LAST="$(tail -1 "$FLEET_DIR/quota.log" 2>/dev/null)"`;
      normalize Python literals first (same sed as Task 1.1:
      `sed 's/:None/:null/g; s/:NaN/:null/g'`); compare against
      `thresholds.json` (same three fields, same
      threshold values hardcoded fallback `75/85/75` if the file is
      missing — a missing config is an anomaly reason `config-missing`,
      not a crash). Never call `quota-snap.sh` and never POST.
   b. Units: same mute rule as Task 1.1 plus failed units:
      `systemctl --user list-units 'fleet-*' --state=failed --no-legend`
      — any line is an anomaly reason `failed <unit>`.
   c. Inbox: same 6 h `*-parked.*`/`*-landed.*` rule — any hit is an
      anomaly reason `stale-inbox <names>`.
   d. Disk: `/home/shared` over 80% is an anomaly reason
      `disk <pct>%`.
   e. If NO anomaly reasons: append
      `<iso8601-utc> GREEN quota=ok units=ok inbox=ok disk=<pct>%`
     to `"$FLEET_DIR/health.log"` and exit 0. No buzzer write, no
     notify-send, no curl.
   f. If anomaly reasons exist (comma-joined into `WHAT`): FIRST append
      `<iso8601-utc> ANOMALY <WHAT>` to `"$FLEET_DIR/health.log"`
      (before any side-effect). Then dedupe:
      `LA="$(cat "$FLEET_DIR/watchdog.last-anomaly" 2>/dev/null || echo 0)"`;
      `NOW="$(date +%s)"`. If `NOW - LA >= 900`: append
      `<iso8601-utc> watchdog-anomaly <WHAT>` to `"$FLEET_DIR/buzzer"`,
      write `NOW` to `watchdog.last-anomaly`, and run
      `notify-send -u critical "fleet watchdog" "<WHAT>" 2>/dev/null || true`.
      If `NOW - LA < 900`: no buzzer, no notify-send. Exit 1 either way.

**Tests:** (same stub harness as Task 1.1; notify-send stub appends its
args to `$T/notified.log`)
- `watchdog-syntax`: `bash -n watchdog.sh` → exit 0; mode 0755.
- `watchdog-green-cycle`: green synthetic quota, no failed-unit stub
  output, empty inbox → `health.log` gains one line matching
  ` GREEN quota=ok units=ok inbox=ok disk=48%`; exit 0; no `buzzer`
  file created; `$T/notified.log` absent.
- `watchdog-anomaly-breach`: synthetic quota `glm_5h_pct":90` →
  `health.log` gains ` ANOMALY ` line containing `glm_5h=90/75`;
  `buzzer` gains one line matching `watchdog-anomaly `; exit 1;
  `$T/notified.log` exists.
- `watchdog-anomaly-failed-unit`: `FAILED_LINE=1` (systemctl stub
  returns `fleet-dead.service loaded failed failed` for
  `--state=failed`) → ANOMALY line contains `failed fleet-dead.service`;
  buzzer written.
- `watchdog-dedupe`: run the breach case twice back-to-back → after run
  2, `buzzer` has exactly ONE line and `health.log` has TWO ANOMALY
  lines; exit 1 both runs.
- `watchdog-disk-breach`: `DF_PCT=85` (df stub returns `85%`) → ANOMALY
  line contains `disk 85%`.
- `watchdog-stale-inbox`: `touch -d '7 hours ago' "$T/inbox/old-parked.md"`
  with otherwise green setup → ANOMALY line contains `stale-inbox
  old-parked.md`.
- `watchdog-python-literal-quota`: quota.log line containing
  `"ocgo_mo_pct":95` and `"anthropic_reset":None` → ANOMALY reason
  contains `ocgo_mo=95/85`, NOT `ocgo_mo=null/85`.

**Acceptance:**
- All 8 tests pass.
- `bash -n` clean; mode 0755.
- Script contains no `curl` invocation at all (grep -c curl = 0).

- [x] 1.2

### Task 1.3: selfwake wake-regex extension + SURGERY.md

**Files:**
- `/home/kenny/dev/rust-camel/.opencode/fleet/selfwake.sh` (modified — exactly one line)
- `/home/kenny/dev/rust-camel/.opencode/fleet/SURGERY.md` (new)

**Steps:**
1. In `selfwake.sh` change the wake grep line from
   `grep -qE "parked|session\.(idle|completed|error|deleted)"` to
   `grep -qE "parked|session\.(idle|completed|error|deleted)|watchdog-anomaly"`.
   Touch NOTHING else in the file.
2. Write `SURGERY.md` with these sections:
   - `# Fleet surgery playbook — quota-starved agents` intro: when to use
     (heartbeat WARN / watchdog ANOMALY / no buzzer from a unit for >30
     min).
   - `## Ladder` (three rungs, in order): (1) quota reset under 2 h away
     → WIP park: agent writes `.opencode/fleet/inbox/<mission>-parked.md`
     with a `resume-at: <iso time>` note; conductor resumes after reset.
     (2) another lane has headroom (check quota digest) → resume the
     SAME session with a different `-m` model — model is a run flag,
     context survives (empirical record below); command shape:
     `systemd-run --user --unit=<unit>-vN <opencode-bin> run --attach
     http://localhost:8080 --dir /home/kenny/dev/rust-camel --session
     <sid> --agent conductor-light --auto --format json -m <other-model>`
     via `resume-mission.sh` gating. Caveat: deep sessions (200k history)
     may degrade on lane hop. (3) all lanes dry → park ALL missions +
     `notify-send -u critical "fleet" "all lanes dry — parked"`.
   - `## session.error wake path`: the SSE listener WAKE_EVENTS already
     wake the master on `session.error` (HTTP 429/530 surface here); the
     SILENT case (eternal retry loop, dead runner, no buzzer) is caught
     by the heartbeat/watchdog mute sweep — that is the backstop.
   - `## Heartbeat receipt protocol (master)`: on `[HEARTBEAT HH:MM]`:
     all OK → one-line reply only; any WARN → apply the ladder to the
     affected lane; acknowledge ANOMALY buzzer lines when woken.
   - `## Thresholds`: the three fields and values, pointer to
     `thresholds.json` as source of truth.
   Do NOT include a verification-record section — Task 2.1 appends the
   live record.

**Tests:**
- `selfwake-wakes-on-watchdog`: `echo "2026-09-23T08:00:00Z watchdog-anomaly quota glm_5h=90/75" | grep -qE "parked|session\.(idle|completed|error|deleted)|watchdog-anomaly"` → exit 0 (match).
- `selfwake-regex-file`: `grep -c 'watchdog-anomaly' selfwake.sh` → exactly 1; `bash -n selfwake.sh` → exit 0.
- `surgery-ladder-complete`: `SURGERY.md` contains strings `resume-at`,
  `-m <other-model>` (grep with `grep -qF -- '-m <other-model>'` — the
  `--` guard is required, bare `-m` parses as grep's max-count),
  `notify-send -u critical`, `session.error`, `[HEARTBEAT HH:MM]`.

**Acceptance:**
- All 3 tests pass.
- `git diff`-equivalent check impossible (gitignored) — instead
  `grep -n 'grep -qE' selfwake.sh` shows the single extended line;
  selfwake.state and all other lines byte-identical apart from it.

- [x] 1.3

## Systemd units (user session)

### Task 1.4: timers + services + selfwake cadence

**Files:**
- `/home/kenny/.config/systemd/user/fleet-heartbeat.service` (new)
- `/home/kenny/.config/systemd/user/fleet-heartbeat.timer` (new)
- `/home/kenny/.config/systemd/user/fleet-watchdog.service` (new)
- `/home/kenny/.config/systemd/user/fleet-watchdog.timer` (new)
- `/home/kenny/.config/systemd/user/fleet-selfwake.timer` (modified)

**Steps:**
1. Write `fleet-heartbeat.service`: `[Unit]
Description=Camel fleet hourly heartbeat battery` and `[Service]
Type=oneshot
ExecStart=/home/kenny/dev/rust-camel/.opencode/fleet/heartbeat.sh`.
2. Write `fleet-heartbeat.timer`: `[Timer] OnCalendar=hourly
Persistent=true` plus `[Install] WantedBy=timers.target` and a
`[Unit] Description=` line.
3. Write `fleet-watchdog.service`: same shape, ExecStart watchdog.sh.
4. Write `fleet-watchdog.timer`: `[Timer] OnBootSec=120
OnUnitActiveSec=300 AccuracySec=5` plus Install section.
5. Edit `fleet-selfwake.timer`: `OnUnitActiveSec=600` → `180` (keep
   `OnBootSec=90`, `AccuracySec=15`).
6. `systemctl --user daemon-reload`, then
   `systemctl --user restart fleet-selfwake.timer` (applies the 180 s
   cadence immediately; selfwake stays quiet unless the buzzer advances).
   Do NOT `enable --now` the new timers here — enabling starts live
   anomaly buzzing (current `ocgo_mo_pct` breach); Task 2.2 owns the
   enable at verification time.

**Tests:**
- `units-load`: `systemctl --user daemon-reload` → exit 0 (this is the
  unit-file validator; `systemctl cat` only prints).
- `heartbeat-timer-cadence`: `systemctl --user cat fleet-heartbeat.timer`
  contains `OnCalendar=hourly` and `Persistent=true`.
- `watchdog-timer-cadence`: `systemctl --user cat fleet-watchdog.timer`
  contains `OnUnitActiveSec=300`.
- `selfwake-timer-180`: `systemctl --user cat fleet-selfwake.timer`
  contains `OnUnitActiveSec=180`.
- `selfwake-restarted`: `systemctl --user is-active fleet-selfwake.timer`
  → `active`.

**Acceptance:**
- All 5 tests pass.
- New timers are loadable but NOT yet active (enable happens in 2.2).

- [x] 1.4

## Live verification (conductor-operated)

### Task 2.1: lane-hop empirical verification on disposable session

**Files:**
- `/home/kenny/dev/rust-camel/.opencode/fleet/SURGERY.md` (modified — append record)

**Steps:**
1. Ground truth (from `inbox/processed-jobpath-parked.md`): mission 229
   jobpath, bd `rc-r63b1`, final tree `b4ce0874`.
2. Precondition: confirm the session id resolves SERVER-side — the
   attach server session list (Basic-auth GET `http://localhost:8080/
   session`) includes `ses_f35f9dc9affe484bt6QY9kmWE1`. The on-disk
   store under `~/.local/share/opencode/storage/` uses hashed names, so
   a filesystem `find` for `ses_…` returning nothing is EXPECTED and is
   not a failure. A server-side miss means a stale id — STOP and report
   instead of spending the lane-hop.
3. Run the disposable session `ses_f35f9dc9affe484bt6QY9kmWE1` (Mission
   229 jobpath execution — finished, parked) on a DIFFERENT lane:
   ```
   systemd-run --user --unit=fleet-lanehop-test \
     /nix/store/6pw7n475sa1d4scq8sy1qkdn2bcy0glc-opencode-1.18.29/bin/opencode run \
     --attach http://localhost:8080 --dir /home/kenny/dev/rust-camel \
     --session ses_f35f9dc9affe484bt6QY9kmWE1 --agent conductor-light \
     --auto --format json -m anthropic/claude-haiku-4-5 \
     "One-line answer, no tools: state the bd issue id and the final commit sha of the mission you executed."
   ```
4. Wait for unit exit (`while systemctl --user is-active --quiet
   fleet-lanehop-test; do sleep 5; done`, 10-minute cap), then capture
   output: `journalctl --user -u fleet-lanehop-test --no-pager`.
5. Append to `SURGERY.md` a section `## Lane-hop verification — live
   record (2026-09-23)` containing: model used
   (`anthropic/claude-haiku-4-5`), the verbatim question, the verbatim
   answer, and a line `Verdict: PASS` when the answer contains both
   `rc-r63b1` and `b4ce0874`, otherwise `Verdict: FAIL`, plus a one-line
   note that a FAIL still validates the playbook (rung 2 gets annotated
   "context does not survive — park instead").
6. Touch NO other session. The disposable session's `session.idle`
   buzzer event afterwards is expected noise.

**Tests:**
- `lanehop-ground-truth`: captured output contains `rc-r63b1` AND
  `b4ce0874` (PASS) — or the mismatch is recorded verbatim (FAIL).
- `lanehop-recorded`: `SURGERY.md` contains
  `## Lane-hop verification — live record` and the model id and a line
  starting `Verdict: PASS` or `Verdict: FAIL`.

**Acceptance:**
- Both tests pass; record is honest (FAIL is acceptable, fabrication is
  not).

- [x] 2.1

### Task 2.2: live cycles — forced heartbeat, real watchdog cycle, green proof

**Files:** none (runtime verification; evidence lands in
`.opencode/fleet/health.log` and `events.log`)

**Steps:**
1. Hermetic green proof: temp dir + synthetic green quota.log + stubs
   (as in Task 1.2 tests) → `PATH`-stubbed `FLEET_DIR=$T watchdog.sh`
   run ends 0 with a GREEN health.log line.
2. Enable the fleet: `systemctl --user enable --now
   fleet-heartbeat.timer fleet-watchdog.timer` — from this moment the
   live `ocgo_mo_pct` breach WILL raise critical notify-sends and a
   `watchdog-anomaly` buzzer line (max one per 15 min); the following
   selfwake SYSTEM-WAKE to the master is the end-to-end wake path being
   proven, not a malfunction.
3. Forced real heartbeat: `systemctl --user start
   fleet-heartbeat.service`; then `tail -3
   /home/kenny/dev/rust-camel/.opencode/fleet/events.log` — expect a
   `{"src":"heartbeat","ev":"post","http":200,...}` line (with
   `"warns"` >= 1 given the live breach — correct real-world behavior).
   The master session receives the digest.
4. Real watchdog cadence: within 6 minutes of enabling, check
   `tail -5 /home/kenny/dev/rust-camel/.opencode/fleet/health.log` for a
   timer-produced line (ANOMALY expected under the live breach).
5. Confirm selfwake cadence: `systemctl --user list-timers` shows
   fleet-selfwake.next within ~3 minutes.

**Tests:**
- `timers-active`: `systemctl --user is-active fleet-heartbeat.timer
  fleet-watchdog.timer fleet-selfwake.timer` → all three print `active`.
- `forced-heartbeat-posts`: events.log gains a heartbeat post line with
  `"http":200`.
- `watchdog-timer-cycle-observed`: health.log gains a line with
  timestamp within the observation window (GREEN in hermetic proof,
  ANOMALY in the live cycle — either proves the timer fires).
- `selfwake-3min`: list-timers NEXT for fleet-selfwake.timer is <= 3 min
  away.

**Acceptance:**
- All 4 tests pass.
- Results recorded verbatim in the park report.

- [x] 2.2

## Addendum — post-bless deviations (accepted, reviewed by stage-4 expert)

Implemented deviations from the blessed task text, all live-verified:

1. Task 1.1/1.2 inbox find commands additionally exclude
   `! -name 'processed-*'` (first live run flagged 195 swept files).
2. Task 1.1/1.2 `systemctl --user list-units` calls use `--plain`
   (live `●` glyph leaked into the failed-unit reason).
3. Task 1.4 service units pin `Environment=PATH=…` (quota-monitor
   pattern) — without python3/git in PATH, quota-snap writes all-null
   lines under systemd.
4. Task 1.4 service units also set `SuccessExitStatus=1` — monitors
   exit 1 on WARN by spec; systemd must not mark them failed or the
   watchdog reports the monitors themselves as failed units (exit >= 2
   still fails).
5. Task 1.3 selfwake.sh header comment updated to 3-min fallback,
   matching the 180 s cadence change.
6. Test counts grew with regression tests: heartbeat 13/13
   (config-bad-keys, curl-000, processed-not-stale), watchdog 11/11
   (config-bad-keys, mute-unit, processed-not-stale).
