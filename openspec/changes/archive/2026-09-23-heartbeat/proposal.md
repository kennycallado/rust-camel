# Proposal: heartbeat

## Why

The fleet wake system is 100% event-driven: nothing watches TIME or overall
fleet health. Quota exhaustion, silent retry loops (dead runner, no buzzer),
stale inbox files, and shared-disk growth stay invisible until a human looks.
bd rc-0k4og (owner-adjudicated full design) specifies the fix: an hourly
heartbeat to the master session, a thin 5-minute anomaly watchdog, and a
quota-starved surgery playbook. Owner quote: "we need a perfect flow" — two
missed-late parks last night were the final trigger.

## What Changes

In scope (4 deliverables, bd rc-0k4og + mission order 235):

1. **Hourly heartbeat**: `.opencode/fleet/heartbeat.sh` +
   `thresholds.json` + user-systemd `fleet-heartbeat.timer` (OnCalendar=hourly,
   Persistent=true). The script runs the battery — quota vs thresholds
   (`glm_5h_pct>75`, `ocgo_mo_pct>85`, `anthropic_7d_pct>75`), unit mute sweep
   (active-but-no-output >30 min = suspect), park/land inbox reports
   (`*-parked.*`/`*-landed.*`) older than 6 h,
   `/home/shared` usage >80% — prints a compact digest (one line per check,
   WARN/OK suffix, exit 0/1), and POSTs `[HEARTBEAT HH:MM] <digest>` to the
   master session via the opencode attach server (selfwake auth pattern).
   All-OK means a one-line master reply only.
2. **Thin watchdog**: `.opencode/fleet/watchdog.sh` +
   `fleet-watchdog.timer` (5 min). Green cycle = one ts line in
   `.opencode/fleet/health.log`, exit 0, zero master tokens. Anomaly =
   ANOMALY line in health.log + buzzer line `watchdog-anomaly <what>` +
   notify-send, deduped to one buzzer line per 15 min via
   `watchdog.last-anomaly`. Requires a one-line regex extension in
   `selfwake.sh` so watchdog buzzer lines wake the master through the existing
   selfwatch path.
3. **Surgery playbook**: `.opencode/fleet/SURGERY.md` — the ladder from bd
   (reset <2 h → WIP park with resume-at note; other lane headroom → resume
   SAME session with different `-m`; all dry → park all + notify-send), the
   `session.error` → wake path note, and an EMPIRICAL lane-hop verification
   run against a disposable finished mission session (jobpath mission
   `ses_f35f9dc9affe484bt6QY9kmWE1`). Active mission sessions are never
   touched.
4. **Timer consolidation**: `fleet-selfwake.timer` 600 s → 180 s.

Out of scope: no Rust/crate changes; no listener.mjs changes; no new quota
sources (reuse `quota-snap.sh` / `quota.log`); no merge (park protocol).

**Placement note (owner order)**: `.opencode/fleet/` is gitignored, so the
fleet scripts/docs land directly in the MAIN checkout (not the worktree) and
are owner-push-only non-assets; systemd timers/units are user-session state
installed live at the end. This worktree carries only the OpenSpec record.

## Acceptance criteria

- Forced heartbeat run posts a digest to the master session; exit code
  matches WARN presence; format is one line per check with WARN/OK suffix.
- One watchdog cycle green: health.log ts line, exit 0, no buzzer write, no
  master POST. (Live quotas currently breach `ocgo_mo_pct` — the green
  demonstration MAY use a temporary threshold override or a synthetic
  quota.log line, restored before park.)
- Simulated anomaly: health.log ANOMALY line + buzzer line + notify-send;
  repeat within 15 min adds no second buzzer line.
- `selfwake.sh` posts a SYSTEM-WAKE for a `watchdog-anomaly` buzzer line.
- SURGERY.md records the ladder and the live lane-hop verification result.
- `fleet-selfwake.timer` active at 180 s.
- All scripts `bash -n` clean (shellcheck unavailable on this host — noted
  as gate N/A).

## Risk budget

- Master-token spend bounded: 1 hourly POST; watchdog zero when green.
- False-positive mute detection on long silent LLM turns: 30-min silence
  threshold per bd design; acceptable.
- Watchdog double-paging capped by the 15-min buzzer dedupe.
- Out of bounds: touching active mission sessions, any `git push`, any
  merge to main.
