#!/usr/bin/env python3
"""Warm M2 exit gate: 24/24 tick-scenario cells with n>0 protocol-B records.

Usage: warm-24.py <run_dir>

<run_dir> is a harness run output directory holding one m2-round-<r>/
subdir per M2 round, each with <scenario>/<contender>/ per cell
(the layout run.sh m2_measure_protocol_b writes).

The 24 tick-scenario cells are the 3 tick scenarios (t2-json,
split-aggregate, t2-realistic-eip) x the 8 full-scenario contenders,
mirroring run.sh SCENARIO_ARTIFACT_SET / summarize.py FULL_CONTENDERS.

A cell passes iff ANY round shows n>0 parsed protocol-B records,
counted from the first source that applies:
  - m2-summary.json -> total_samples (harness success path);
  - m2-summary.txt  -> observed=N on the
    status=failed reason=insufficient-samples line. That status is
    PRESENT data for this gate (the cell ticked, just below the
    publication minimum) — this gate asserts presence, not
    publication readiness, so the status itself is not a failure.
    Legacy fallback: since the adaptive m2 window (bench-consol-tick
    task 3.2, bd rc-tpig) run.sh emits that status only with
    observed=0 (dead cell), which yields n=0 and FAILS this gate;
    observed>0 lines only exist in pre-fix run dirs.
  - status=not-measured reason=no-protocol-b-records, or no summary
    at all -> n=0 (fail).

Exits 0 iff all 24 cells have n>0; otherwise prints every offending
cell and exits 1. Stdlib only, deterministic.
"""

import argparse
import json
import re
import sys
from pathlib import Path

# Mirror of run.sh SCENARIO_ARTIFACT_SET ("full" set) restricted to the
# three tick scenarios (change bench-consol-tick task 2.8).
TICK_SCENARIOS = ("split-aggregate", "t2-json", "t2-realistic-eip")
FULL_CONTENDERS = (
    "camel-quarkus-dsl-native",
    "camel-quarkus-yaml-native",
    "camel-standalone-dsl",
    "camel-standalone-yaml",
    "node-fastify",
    "node-native",
    "rust-camel-cli",
    "rust-camel-lib",
)

_ROUND_DIR = re.compile(r"^m2-round-(\d+)$")
_OBSERVED = re.compile(r"observed=(\d+)")


def cell_records(run_dir: Path, scenario: str, contender: str) -> tuple[int, str]:
    """Max record count for a cell across rounds + a detail note."""
    best = 0
    notes: list[str] = []
    for round_dir in sorted(run_dir.glob("m2-round-*")):
        if not _ROUND_DIR.match(round_dir.name):
            continue
        cell_dir = round_dir / scenario / contender
        summary_json = cell_dir / "m2-summary.json"
        summary_txt = cell_dir / "m2-summary.txt"
        n = 0
        if summary_json.is_file():
            try:
                data = json.loads(summary_json.read_text())
                n = int(data.get("total_samples", 0))
            except (ValueError, TypeError, OSError):
                n = 0
                notes.append(f"{round_dir.name}: unparseable m2-summary.json")
        elif summary_txt.is_file():
            text = summary_txt.read_text()
            match = _OBSERVED.search(text)
            if "status=failed" in text and match:
                n = int(match.group(1))
            else:
                reason = "no-protocol-b-records" in text
                notes.append(f"{round_dir.name}: "
                             + ("not-measured (no records)" if reason
                                else "status line without observed=N"))
        else:
            notes.append(f"{round_dir.name}: no summary written")
        best = max(best, n)
    return best, "; ".join(notes)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("run_dir", type=Path,
                    help="harness run dir holding m2-round-*/ subdirs")
    args = ap.parse_args()

    if not args.run_dir.is_dir():
        print(f"error: run dir not found: {args.run_dir}", file=sys.stderr)
        return 1
    if not any(_ROUND_DIR.match(p.name) for p in args.run_dir.iterdir()):
        print(f"error: no m2-round-*/ dirs under {args.run_dir} — pass the "
              f"inner run dir (out/<ts>/<inner-ts>), not the out/<ts> parent",
              file=sys.stderr)
        return 1

    offenders: list[tuple[str, int, str]] = []
    total = 0
    for scenario in TICK_SCENARIOS:
        for contender in FULL_CONTENDERS:
            total += 1
            cell = f"{scenario}_{contender}"
            n, note = cell_records(args.run_dir, scenario, contender)
            status = "ok" if n > 0 else "FAIL"
            line = f"{cell}: n={n} [{status}]"
            if note:
                line += f" ({note})"
            print(line)
            if n <= 0:
                offenders.append((cell, n, note))

    print(f"--- warm gate: {total - len(offenders)}/{total} cells with n>0")
    if offenders:
        for cell, _n, note in offenders:
            print(f"offending cell: {cell}: no parsed protocol-B records"
                  + (f" ({note})" if note else ""))
        return 1
    print("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
