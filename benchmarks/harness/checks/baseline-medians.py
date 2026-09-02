#!/usr/bin/env python3
"""Compute the per-cell median M1 time-to-marker baseline from a run dir.

Usage: baseline-medians.py <run_dir> <out_json> --source-run <id>

<run_dir> is a harness run output directory holding one <scenario>_<contender>/
subdir per cell, each with samples.txt (rows: "<ms> <rss_kb>" — the M1
time-to-marker in ms is the FIRST column; the rss column is ignored here,
the median is over ms only).

Emits {"source_run": <id>, "cells": {"<cell>": {"median_ms": <float>}}}
with one entry per cell. Every cell must have exactly --n samples
(default 30); a short cell exits non-zero without writing <out_json>.

Task 2.1 of change bench-consol-tick captured
openspec/changes/bench-consol-tick/pre-tick-baseline.json with this
script BEFORE the tick-mode changes of Tasks 2.2+ touched any fixture.
"""

import argparse
import json
import statistics
import sys
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("run_dir", type=Path)
    ap.add_argument("out_json", type=Path)
    ap.add_argument("--source-run", required=True,
                    help="run id recorded in the output (meta.json run_id)")
    ap.add_argument("--n", type=int, default=30,
                    help="required sample count per cell (default 30)")
    args = ap.parse_args()

    if not args.run_dir.is_dir():
        print(f"error: run dir not found: {args.run_dir}", file=sys.stderr)
        return 1

    cells: dict[str, dict[str, float]] = {}
    for cell_dir in sorted(args.run_dir.iterdir()):
        samples = cell_dir / "samples.txt"
        if not samples.is_file():
            continue
        rows = [ln.split() for ln in
                samples.read_text().splitlines() if ln.strip()]
        if len(rows) != args.n:
            print(f"error: {cell_dir.name}: expected {args.n} samples, "
                  f"got {len(rows)}", file=sys.stderr)
            return 1
        ms = [float(r[0]) for r in rows]
        cells[cell_dir.name] = {"median_ms": statistics.median(ms)}

    if not cells:
        print("error: no cells found in run dir", file=sys.stderr)
        return 1

    args.out_json.write_text(
        json.dumps({"source_run": args.source_run, "cells": cells},
                   indent=2) + "\n")
    print(f"wrote {len(cells)} cells to {args.out_json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
