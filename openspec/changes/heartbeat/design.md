# Design: heartbeat

## Approach

Reuse the selfwake.sh mechanism set — no new daemons, oneshot systemd units
only: `server-auth` password file, `conductor.json` master id, Basic-auth
POST to `http://localhost:8080/session/<master>/message`, JSON trail in
`events.log`.

### heartbeat.sh (hourly)

1. Run `bash quota-snap.sh` first — refreshes `quota.log` so the battery sees
   near-current numbers (the existing `quota-monitor.timer` keeps its hourly
   slot; the overlap is a harmless duplicate append).
2. Battery — each check emits exactly one digest line, `WARN` or `OK`
   suffix; any WARN forces exit 1:
   - **quota**: `jq` the last `quota.log` line, compare against
     `thresholds.json` (`glm_5h_pct: 75`, `ocgo_mo_pct: 85`,
     `anthropic_7d_pct: 75`). A `null` field counts as WARN-stale (no data).
   - **units**: for each running `fleet-*` unit, take the last journal entry
     timestamp (`journalctl --user -u <unit> -n 1`); a unit with no output
     for >30 min AND active for >30 min is mute → WARN with unit name and
     silence age. Otherwise an OK summary line with the running count.
   - **inbox**: `find .opencode/fleet/inbox -maxdepth 1 -type f \( -name
     '*-parked.*' -o -name '*-landed.*' \) ! -name 'processed-*' -mmin
     +360` — count > 0 → WARN with names. (Reports awaiting the normal
     conductor sweep are in-flight state, not staleness; 6 h = one full
     sweep-cycle margin. The naive "not processed-* and >1 h" rule flags
     ~52 healthy files today — rejected. The `! -name 'processed-*'`
     guard was added post-bless after the first live run flagged 195
     already-swept `processed-*-parked.*` files — see Post-bless
     deviations.)
   - **disk**: `df -P /home/shared` usage >80% → WARN with the percentage.
   - **failure paths**: missing/unreadable `thresholds.json` → one
     `WARN config: thresholds.json missing` line, exit 1, no `jq` crash
     (guard before parsing). Empty/missing `quota.log` or `null` field →
     WARN-stale. POST failure (non-2xx) does not change the exit code; the
     real HTTP code lands in `events.log`.
3. POST `[HEARTBEAT HH:MM]` + digest to the master session (same curl +
   `-u opencode:$PASS` shape as selfwake.sh); append
   `{"src":"heartbeat","ev":"post","http":N,"warns":N}` to `events.log`.
   Script exit code = 0 all-OK / 1 any-WARN.

Master behavior on receipt (documented in SURGERY.md): read the digest; all
OK → one-line reply; WARN → act per the surgery ladder.

### watchdog.sh (every 5 min)

- Reads the LAST `quota.log` line only — no provider API calls (thin).
- Same four conditions, plus failed units: `systemctl --user --state=failed`
  matching `fleet-*`.
- Green: append `<iso-ts> GREEN <one-line summary>` to `health.log`,
  exit 0. No buzzer write, no master POST, no notify-send.
- Anomaly: append `<iso-ts> ANOMALY <what>` to `health.log`; write buzzer
  line `watchdog-anomaly <what>` and fire `notify-send -u critical` ONLY if
  `watchdog.last-anomaly` (epoch ts file) is older than 900 s — then update
  it. Repeated anomalies within 15 min keep logging ANOMALY lines but never
  double-buzz.
- Never POSTs to the master directly — buzzer + extended selfwake regex is
  the wake path.
- Ordering: `health.log` append happens BEFORE any buzzer/notify
  side-effect; a failed buzzer write or notify-send does not change the
  exit code.

### selfwake.sh change (one line)

Wake grep regex `"parked|session\.(idle|completed|error|deleted)"` gains
`watchdog-anomaly`. Keeps the single state-gated wake path; no other edits.

### SURGERY.md

Sections: (1) the ladder — reset <2 h → WIP park with resume-at note; other
lane headroom → resume SAME session with different `-m` (model is a run
flag, context survives); all dry → park all + notify-send. (2) The
`session.error` → wake path note: listener WAKE_EVENTS already wakes the
master on session.error (429/530); the silent eternal-retry case is caught
by the heartbeat/watchdog mute sweep. (3) Lane-hop verification record:
disposable target `ses_f35f9dc9affe484bt6QY9kmWE1` (Mission 229 jobpath,
finished and parked), resumed in a transient systemd unit with a DIFFERENT
`-m` model, asked one factual question answerable only from session context
(bd id + final commit of its mission); record model id, answer, and
context-survival verdict.

### Timer consolidation

`fleet-selfwake.timer`: `OnUnitActiveSec` 600 → 180 (keep `OnBootSec=90`,
`AccuracySec=15`). Watchdog complements selfwake (buzzer dedupe + mute
detection), it does not replace it.

## Affected crates

None — no Rust code, no Cargo.toml edits. The repo diff contains only
`openspec/changes/heartbeat/`. All cargo gates are N/A under registry
N/A-detection (zero `.rs`/`Cargo.toml` in diff).

## Architecture boundaries

Fleet ops layer, outside the data/control plane. No crate boundary is
crossed. The only repo-visible surface is the OpenSpec record; the live
deployment targets are the gitignored `.opencode/fleet/` directory and the
user systemd instance.

## Alternatives considered

- **Watchdog POSTs master directly on anomaly** — rejected: the order pins
  the buzzer path; it reuses the existing state-gated wake and dedupe
  machinery instead of a second POST path.
- **Watchdog runs quota-snap each cycle** — rejected: 12 extra provider hits
  per hour for numbers that move hourly; reading the last line bounds
  staleness at ~1 h and the heartbeat refreshes on the hour.
- **Extend listener.mjs with timer events** — rejected: the SSE bus has no
  timer domain; systemd is already the timer authority.
- **Cron instead of systemd timers** — rejected: the fleet pattern is user
  systemd units; `Persistent=true` catch-up after suspend comes free.

Single-phase change: one coherent slice, no milestone grouping.

## Post-bless deviations (accepted during implementation, all reviewed)

1. **Inbox find gains `! -name 'processed-*'`** (both scripts). The
   `*-parked.*` glob alone also matched `processed-*-parked.*` history —
   the first live run flagged 195 already-swept files. Aligns the
   implementation with the spec prose ("normal in-flight reports … are
   NOT stale" — swept history even less so).
2. **`--plain` on all `systemctl --user list-units` calls** (both
   scripts). Live legend output carries a `●` glyph that leaked into
   the failed-unit anomaly reason.
3. **`Environment=PATH=…` pinned into both service units** (the
   quota-monitor.service line). Without it the systemd user PATH lacks
   python3/git and quota-snap.sh appends all-null lines.
4. **`SuccessExitStatus=1` on both service units.** The monitors exit 1
   on health WARNs by spec; without this, systemd marks them failed and
   the watchdog's failed-unit check false-anomalies on the monitors
   themselves. Exit >= 2 still counts as failure.
5. **`selfwake.sh` header comment** updated 10-min → 3-min fallback to
   match the 180 s timer (beside the blessed one-line regex edit).
6. **tasks.md** post-bless edits: pinned grep command fixed with `--`
   guard (r_glm finding — it parsed `-m` as grep's max-count);
   checkbox marks; addendum section (this list's counterpart).
