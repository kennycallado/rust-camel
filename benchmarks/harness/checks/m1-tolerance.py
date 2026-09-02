#!/usr/bin/env python3
"""M1 tolerance gate: post-tick cold-start medians vs pre-tick baseline.

Usage: m1-tolerance.py <pre_json> <post_run_dir> [--n 30]

<pre_json> is the pre-tick baseline (pre-tick-baseline.json, produced
by baseline-medians.py). <post_run_dir> is a post-change M1 run dir
(flat <scenario>_<contender>/samples.txt subdirs, first column = the
M1 time-to-marker in ms — the same shape baseline-medians.py reads).

Passes iff EVERY baseline cell satisfies the blessed tolerance
    abs(post - pre) <= max(0.15 * pre, 3.0)  ms
with post = median of exactly --n samples (default 30, enforced).
On failure prints EVERY offending cell with pre/post/delta/tolerance
and exits 1; extra run-dir cells absent from the baseline are
reported as warnings. Stdlib only, deterministic.
"""

import argparse
import json
import statistics
import sys
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("pre_json", type=Path)
    ap.add_argument("post_run_dir", type=Path)
    ap.add_argument("--n", type=int, default=30,
                    help="required sample count per cell (default 30)")
    args = ap.parse_args()

    if not args.pre_json.is_file():
        print(f"error: baseline not found: {args.pre_json}", file=sys.stderr)
        return 1
    if not args.post_run_dir.is_dir():
        print(f"error: run dir not found: {args.post_run_dir}",
              file=sys.stderr)
        return 1

    baseline = json.loads(args.pre_json.read_text())["cells"]

    violations: list[str] = []
    rows: list[tuple[str, float, float, float, float, bool]] = []
    seen: set[str] = set()
    for cell, entry in sorted(baseline.items()):
        pre = float(entry["median_ms"])
        samples = args.post_run_dir / cell / "samples.txt"
        if not samples.is_file():
            violations.append(f"{cell}: missing samples.txt "
                              f"(no cell dir in post run)")
            rows.append((cell, pre, float("nan"), float("nan"),
                         max(0.15 * pre, 3.0), False))
            continue
        raw = [ln.split() for ln in samples.read_text().splitlines()
               if ln.strip()]
        if len(raw) != args.n:
            violations.append(f"{cell}: expected {args.n} samples, "
                              f"got {len(raw)}")
            rows.append((cell, pre, float("nan"), float("nan"),
                         max(0.15 * pre, 3.0), False))
            continue
        post = statistics.median(float(r[0]) for r in raw)
        tol = max(0.15 * pre, 3.0)
        delta = post - pre
        ok = abs(delta) <= tol
        rows.append((cell, pre, post, delta, tol, ok))
        seen.add(cell)
        if not ok:
            violations.append(
                f"{cell}: pre={pre:.1f} post={post:.1f} "
                f"delta={delta:+.1f} tol={tol:.2f} (|delta| > tol)")

    extra = sorted(p.name for p in args.post_run_dir.iterdir()
                   if p.is_dir() and p.name not in baseline)
    for name in extra:
        print(f"warning: run-dir cell not in baseline (ignored): {name}",
              file=sys.stderr)

    print(f"{'cell':<48} {'pre':>8} {'post':>8} {'delta':>8} {'tol':>7}  ok")
    for cell, pre, post, delta, tol, ok in rows:
        post_s = f"{post:8.1f}" if post == post else "     n/a"
        delta_s = f"{delta:+8.1f}" if delta == delta else "     n/a"
        print(f"{cell:<48} {pre:8.1f} {post_s} {delta_s} {tol:7.2f}  "
              f"{'OK' if ok else 'VIOLATION'}")

    if violations:
        print(f"--- m1 tolerance gate: {len(violations)} violation(s)")
        for v in violations:
            print(f"offending cell: {v}")
        return 1
    print(f"--- m1 tolerance gate: {len(rows)}/{len(rows)} cells within "
          f"max(0.15*pre, 3.0) ms")
    print("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
