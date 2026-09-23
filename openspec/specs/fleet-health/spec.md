# fleet-health Specification

## Purpose
TBD - created by archiving change heartbeat. Update Purpose after archive.
## Requirements
### Requirement: Hourly heartbeat battery

The fleet SHALL run an hourly heartbeat (systemd timer, hourly calendar,
persistent catch-up) that gathers quota, unit-mute, inbox-staleness, and
shared-disk checks into a compact digest and posts it to the conductor
session. Each check contributes exactly one digest line with a WARN or OK
suffix; the script exits 0 when all checks pass and 1 when any check warns.
The target session id is read from `.opencode/fleet/conductor.json`
(`.conductor`) at run time, same as `selfwake.sh` — never hardcoded.

Thresholds live in `.opencode/fleet/thresholds.json`:
`glm_5h_pct` 75, `ocgo_mo_pct` 85, `anthropic_7d_pct` 75 (no `gpt_*`
fields are gated). Quota values are read from the last `quota.log` line
after a fresh `quota-snap.sh` run. A missing or unreadable
`thresholds.json` produces one `WARN config: thresholds.json missing`
digest line and exit 1 — the script must not crash. An empty or missing
`quota.log`, or a `null` quota field, counts as WARN-stale (no data). A
failed digest POST (non-2xx) does not change the exit code (still by WARN
presence) but the real HTTP code is recorded in `events.log`.
A mute unit is a running `fleet-*` unit with no journal output for more than
30 minutes while active for more than 30 minutes. A stale inbox file is a
file directly in `.opencode/fleet/inbox/` matching the report pattern
(`*-parked.*` or `*-landed.*`) with mtime older than 6 hours — normal
in-flight reports awaiting the conductor sweep are NOT stale. The disk
check warns when `/home/shared` usage exceeds 80%.

#### Scenario: all-OK heartbeat

- **GIVEN** quota fields below thresholds, no mute units, no park/land
  report in `inbox/` older than 6 hours, and `/home/shared` below 80%
- **WHEN** the heartbeat service runs
- **THEN** the conductor session receives one `[HEARTBEAT HH:MM]` message
  containing one OK line per check and the script exits 0

#### Scenario: config failure warns without crashing

- **GIVEN** `thresholds.json` missing from `.opencode/fleet/`
- **WHEN** the heartbeat battery runs
- **THEN** the digest carries one `WARN config` line, no subcommand crash
  aborts the battery, and the script exits 1

#### Scenario: threshold breach warns

- **GIVEN** the last `quota.log` line has `glm_5h_pct` above 75
- **WHEN** the heartbeat battery runs
- **THEN** the digest carries a quota line with WARN suffix naming the
  breached field and value, and the script exits 1

#### Scenario: mute unit warns

- **GIVEN** a running `fleet-*` unit with no journal output for over 30
  minutes
- **WHEN** the heartbeat battery runs
- **THEN** the digest carries a units line with WARN suffix naming the unit
  and its silence age

### Requirement: Thin watchdog with zero-cost green cycles

The fleet SHALL evaluate the same health conditions every 5 minutes without
consuming conductor tokens when green. A green cycle appends one timestamped
GREEN line to `.opencode/fleet/health.log` and exits 0 — no buzzer write, no
master POST, no notification. An anomalous cycle (threshold breach, mute
unit, failed `fleet-*` unit, stale park/land report, or disk over 80%)
appends an ANOMALY line to `health.log` and raises attention exactly once
per 15 minutes: a buzzer line `watchdog-anomaly <what>` plus a critical
`notify-send`, deduplicated via the `watchdog.last-anomaly` timestamp file.
The `health.log` append happens before any buzzer/notify side-effect, and a
failed side-effect does not change the exit code.

#### Scenario: green cycle

- **GIVEN** all conditions within bounds
- **WHEN** the watchdog service runs
- **THEN** `health.log` gains one timestamped GREEN line, the script exits
  0, and the buzzer is untouched

#### Scenario: anomaly raises attention

- **GIVEN** a `fleet-*` unit in failed state
- **WHEN** the watchdog service runs
- **THEN** `health.log` gains an ANOMALY line, the buzzer gains a
  `watchdog-anomaly <what>` line, and notify-send fires

#### Scenario: repeated anomaly is deduplicated

- **GIVEN** the same anomaly persists and the last buzzer write was 5
  minutes ago
- **WHEN** the watchdog service runs again
- **THEN** `health.log` gains another ANOMALY line but no new buzzer line
  is written and notify-send does not fire

### Requirement: Selfwake wake-regex covers watchdog anomalies

`selfwake.sh` SHALL post a SYSTEM-WAKE to the conductor session when new
buzzer lines contain `watchdog-anomaly`, in addition to the existing wake
patterns (`parked`, `session.idle|completed|error|deleted`), preserving the
single state-gated wake path.

#### Scenario: watchdog buzzer line triggers selfwake

- **GIVEN** `selfwake.state` behind the buzzer line count
- **WHEN** a new buzzer line `watchdog-anomaly unit-failed fleet-x` appears
- **THEN** `selfwake.sh` POSTs a SYSTEM-WAKE to the master session and
  advances its state so the line is not re-processed

### Requirement: Quota-starved surgery playbook

`.opencode/fleet/SURGERY.md` SHALL document the surgery ladder (quota reset
under 2 h → WIP park with resume-at note; another lane has headroom →
resume the SAME session with a different `-m` model; all lanes dry → park
all plus notify-send), the `session.error` wake-path note, and a
lane-hop verification record obtained empirically from a disposable
finished mission session resumed with a different `-m` model and asked one
factual question answerable only from that session's context.

#### Scenario: lane-hop verification recorded

- **GIVEN** the disposable finished jobpath session
- **WHEN** it is resumed with a different `-m` model and asked one factual
  question about its own mission
- **THEN** `SURGERY.md` records the model id used, the question, the answer,
  and a context-survival verdict

### Requirement: Selfwatch cadence 180 seconds

`fleet-selfwake.timer` SHALL fire every 180 seconds so parked-report
landings and watchdog anomalies reach the master within one 3-minute
window.

#### Scenario: timer cadence

- **GIVEN** the consolidated timer is installed and active
- **WHEN** its unit definition is inspected
- **THEN** `OnUnitActiveSec=180` is set and the timer is enabled

