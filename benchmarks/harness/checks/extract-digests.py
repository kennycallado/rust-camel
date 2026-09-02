#!/usr/bin/env python3
"""Extract the per-cell BENCH_INPUT_SHA256 digest oracle from a benchmark run.

Usage: extract-digests.py <run_dir> <out_json>

<run_dir> is a harness run output directory (benchmarks/harness/out/<ts>/)
whose scratch/ holds the per-cell stdout evidence files. M1 evidence naming
(run.sh measure_once): scratch/<scenario>_<contender>.<run_id>.out. Aux
files (m2.*.<ns>.out, devnull-baseline.out) are ignored — the oracle is an
M1 startup property.

Emits {"<scenario>_<contender>": "<sha256>", ...} — one entry per cell that
produced evidence. Every fixture prints its input digest at startup, so the
value is a property of the per-scenario shared payload files, not of the
contender; all runs of one cell within a run directory must therefore agree.
A disagreement (or zero evidence) exits non-zero without writing <out_json>.

Task 1.2 of change bench-consol-tick captures the committed oracle
openspec/changes/bench-consol-tick/pre-move-digests.json with the OLD
per-scenario fixtures in place; Task 1.7's parity check compares the
post-move run against it.
"""

import json
import re
import sys
from pathlib import Path

DIGEST_RE = re.compile(r"BENCH_INPUT_SHA256=([0-9a-f]{64})(?![0-9a-f])")
# M1 evidence files: <scenario>_<contender>.<run_id>.out (cell names are
# lowercase/dash, joined by one underscore; run_id is numeric with dash
# separators, e.g. 1788208386489331214-264-4855).
EVIDENCE_RE = re.compile(r"^(?P<cell>[a-z0-9-]+_[a-z0-9-]+)\.(?P<run>[0-9][0-9-]*)\.out$")


def extract_cell_digest(evidence: Path) -> str | None:
    """First BENCH_INPUT_SHA256= line in one evidence file, or None."""
    for line in evidence.read_text(errors="replace").splitlines():
        m = DIGEST_RE.search(line)
        if m:
            return m.group(1)
    return None


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    run_dir, out_json = Path(sys.argv[1]), Path(sys.argv[2])
    scratch = run_dir / "scratch"
    if not scratch.is_dir():
        print(f"error: no scratch/ directory under {run_dir}", file=sys.stderr)
        return 1

    per_cell: dict[str, dict[str, str]] = {}
    for evidence in sorted(scratch.glob("*.out")):
        m = EVIDENCE_RE.match(evidence.name)
        if not m:
            continue
        digest = extract_cell_digest(evidence)
        if digest is None:
            continue
        per_cell.setdefault(m["cell"], {})[m["run"]] = digest

    conflicts = {c: r for c, r in per_cell.items() if len(set(r.values())) > 1}
    if conflicts:
        for cell, runs in sorted(conflicts.items()):
            print(f"error: digest conflict for {cell}: {runs}", file=sys.stderr)
        return 1
    if not per_cell:
        print(f"error: no BENCH_INPUT_SHA256 evidence under {scratch}", file=sys.stderr)
        return 1

    digests = {cell: next(iter(runs.values())) for cell, runs in sorted(per_cell.items())}
    out_json.write_text(json.dumps(digests, indent=2, sort_keys=True) + "\n")
    print(f"wrote {len(digests)} cell digests to {out_json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
