#!/usr/bin/env python3
"""Phase A exit gate (bench-consol-tick task 1.7): smoke parity of all 52 cells.

Usage: parity-52.py <run_dir> <oracle_json>

<run_dir> is a harness run output directory as produced by
`bash benchmarks/bench run-all` (benchmarks/harness/out/<ts>/):

  meta.json     launch-time meta (protocol.order_seed, scenarios roster)
  scratch/      per-cell stdout evidence <cell>.<run_id>.out — ONE FILE PER
                RUN (3 warmup + n measured for M1), plus .time/.txt aux files
                that never match the evidence naming
  <inner-ts>/   RUN_DIR — one sample dir <scenario>_<contender>/ per cell,
                each holding samples.txt (one "elapsed_ms rss_kb" line per
                successful measured run)

Asserts (ALL failures are collected and listed, then exit 1; exit 0
prints a one-line summary per check group):

  cells    exactly 52 sample dirs (dirs with a samples.txt under the run
           dir), each samples.txt non-empty with >=3 samples
  markers  every scratch .out evidence file of every cell contains the
           scenario's DISTINCTIVE marker string exactly once. The bare
           BENCH_ROUTE_READY substring is never used for suffixed
           scenarios (t2-realistic-eip emits the bare substring twice
           plus its "BENCH_ROUTE_READY body=pong-bench" line once) — the
           per-scenario strings are parsed from the harness's own
           SCENARIO_MARKER wiring in run.sh, never duplicated by hand.
  digests  for the digest-bearing cells (split-aggregate + t2-json, the
           16 oracle entries): all runs of one cell agree, all
           contenders of one scenario agree (cross-contender parity —
           the digest is a property of the per-scenario shared payload
           files), and every cell equals the committed pre-move oracle
           (captured with the OLD fixture layout; the payload files did
           not move, so equality is expected). Cells outside the oracle
           are not digest-asserted.
  meta     protocol.order_seed is present and an integer.

Stdlib-only, deterministic, read-only over <run_dir>.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

EXPECTED_CELLS = 52  # 5 full scenarios x 8 + 2 bridge scenarios x 6 (run.sh guard)
MIN_SAMPLES = 3  # acceptance: n=3 smoke, >=3 samples per cell

DIGEST_RE = re.compile(r"BENCH_INPUT_SHA256=([0-9a-f]{64})(?![0-9a-f])")
# M1 evidence naming (run.sh measure_once): <scenario>_<contender>.<run_id>.out
# — identical to checks/extract-digests.py so both tools agree on what is
# evidence. run_id is numeric with dash separators; aux files (m2.*.<ns>.out,
# devnull-baseline.out, *.time, *.txt) never match.
EVIDENCE_RE = re.compile(r"^(?P<cell>[a-z0-9-]+_[a-z0-9-]+)\.(?P<run>[0-9][0-9-]*)\.out$")


def parse_scenario_markers(run_sh: Path) -> dict[str, str]:
    """Extract the per-scenario marker table from run.sh itself.

    Handles both the literal SCENARIO_MARKER=( ...) block and the
    derived t2-json entry whose bytes suffix is arithmetic over the
    BENCH_PAYLOAD_BYTES default. Any unparseable piece is a hard error:
    guessing marker strings here would weaken the gate.
    """
    text = run_sh.read_text()
    markers: dict[str, str] = {}

    block_re = re.compile(r"declare -A SCENARIO_MARKER=\(\n(.*?)\n\)", re.DOTALL)
    block = block_re.search(text)
    if not block:
        raise SystemExit("error: SCENARIO_MARKER block not found in run.sh")
    for name, value in re.findall(r'\["([^"]+)"\]="([^"]*)"', block.group(1)):
        markers[name] = value

    derived_re = re.compile(r'^SCENARIO_MARKER\["t2-json"\]="(.*)"$', re.MULTILINE)
    derived = derived_re.search(text)
    if not derived:
        raise SystemExit("error: derived t2-json marker assignment not found in run.sh")
    addend_re = re.compile(r"bytes=\$\(\(BENCH_PAYLOAD_BYTES \+ (\d+)\)\)")
    addend = addend_re.search(derived.group(1))
    default_re = re.compile(r'BENCH_PAYLOAD_BYTES="\$\{BENCH_PAYLOAD_BYTES:-(\d+)\}"')
    default = default_re.search(text)
    if not addend or not default:
        raise SystemExit("error: cannot derive t2-json marker bytes from run.sh")
    markers["t2-json"] = addend_re.sub(
        "bytes=%d" % (int(default.group(1)) + int(addend.group(1))), derived.group(1)
    )
    return markers


def find_sample_root(run_dir: Path) -> Path:
    """Locate the dir holding the per-cell sample dirs.

    run-all.sh sets BENCH_RESULTS_ROOT=out/<ts>/ and run.sh nests its run
    dir one level deeper (out/<ts>/<inner-ts>/). When both timestamps land
    in the same second the two collapse. Accept exactly one candidate.
    """
    candidates = [run_dir] if _has_cell_dirs(run_dir) else []
    candidates += [p for p in sorted(run_dir.iterdir()) if p.is_dir() and p.name != "scratch" and _has_cell_dirs(p)]
    if len(candidates) != 1:
        raise SystemExit(
            f"error: expected exactly one sample root under {run_dir}, found "
            f"{[str(c) for c in candidates] or 'none'}"
        )
    return candidates[0]


def _has_cell_dirs(d: Path) -> bool:
    return d.is_dir() and any(c.is_dir() and (c / "samples.txt").is_file() for c in d.iterdir())


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    run_dir, oracle_path = Path(sys.argv[1]), Path(sys.argv[2])

    failures: list[str] = []

    # ---- inputs -------------------------------------------------------
    if not run_dir.is_dir():
        print(f"error: run dir not found: {run_dir}", file=sys.stderr)
        return 1
    scratch = run_dir / "scratch"
    if not scratch.is_dir():
        failures.append(f"layout: no scratch/ directory under {run_dir}")
        scratch = None  # type: ignore[assignment]

    try:
        meta = json.loads((run_dir / "meta.json").read_text())
    except (OSError, json.JSONDecodeError) as exc:
        meta = None
        failures.append(f"meta: cannot read {run_dir}/meta.json: {exc}")

    order_seed = None
    if meta is not None:
        raw_seed = (meta.get("protocol") or {}).get("order_seed")
        if isinstance(raw_seed, int) and not isinstance(raw_seed, bool):
            order_seed = raw_seed
        else:
            failures.append(f"meta: protocol.order_seed missing or not an integer: {raw_seed!r}")
    scenarios = [s for s in (meta or {}).get("scenarios", "").split(",") if s]
    if not scenarios:
        failures.append("meta: scenarios roster empty — cannot map cells to scenarios")

    try:
        markers = parse_scenario_markers(Path(__file__).resolve().parent.parent / "run.sh")
    except SystemExit as exc:
        print(exc, file=sys.stderr)
        return 1

    try:
        oracle = json.loads(oracle_path.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        print(f"error: cannot read oracle {oracle_path}: {exc}", file=sys.stderr)
        return 1

    # ---- cells + samples ----------------------------------------------
    sample_root = find_sample_root(run_dir)
    cells = sorted(p.name for p in sample_root.iterdir() if p.is_dir() and (p / "samples.txt").is_file())
    if len(cells) != EXPECTED_CELLS:
        failures.append(f"cells: expected {EXPECTED_CELLS} sample dirs, found {len(cells)}")

    def scenario_of(cell: str) -> str | None:
        matches = [s for s in scenarios if cell.startswith(s + "_")]
        return matches[0] if len(matches) == 1 else None

    sample_counts: dict[str, int] = {}
    for cell in cells:
        samples_file = sample_root / cell / "samples.txt"
        lines = [l for l in samples_file.read_text(errors="replace").splitlines() if l.strip()]
        sample_counts[cell] = len(lines)
        if len(lines) < MIN_SAMPLES:
            failures.append(
                f"samples: {cell}: expected >={MIN_SAMPLES} non-empty samples, got {len(lines)} "
                f"({samples_file})"
            )
        if scenario_of(cell) is None:
            failures.append(f"cells: {cell}: scenario prefix matches none of the roster {scenarios}")

    # ---- markers: once per run, distinctive string per scenario --------
    evidence: dict[str, list[Path]] = {}
    if scratch is not None:
        for ev in sorted(scratch.glob("*.out")):
            m = EVIDENCE_RE.match(ev.name)
            if m:
                evidence.setdefault(m["cell"], []).append(ev)

    cell_digests: dict[str, set[str]] = {c: set() for c in cells}
    for cell in cells:
        scenario = scenario_of(cell)
        evs = evidence.get(cell, [])
        if not evs:
            failures.append(f"markers: {cell}: no scratch .out evidence found")
            continue
        expected_marker = markers.get(scenario or "")
        if expected_marker is None:
            if scenario is not None:
                failures.append(
                    f"markers: {cell}: no marker wired in run.sh for scenario {scenario!r}"
                )
            continue  # roster mismatch already reported above
        for ev in evs:
            lines = ev.read_text(errors="replace").splitlines()
            marker_count = sum(1 for l in lines if expected_marker in l)
            if marker_count != 1:
                failures.append(
                    f"markers: {cell}: expected exactly 1 {expected_marker!r} line in "
                    f"{ev.name}, got {marker_count}"
                )
            for line in lines:
                d = DIGEST_RE.search(line)
                if d:
                    cell_digests[cell].add(d.group(1))

    # ---- digests: within-cell, cross-contender, oracle -----------------
    oracle_missing = [c for c in oracle if c not in cell_digests]
    if oracle_missing:
        failures.append(f"digests: oracle cells with no run evidence: {oracle_missing}")
    for cell, want in sorted(oracle.items()):
        got = cell_digests.get(cell, set())
        if len(got) == 0:
            continue  # already reported as missing evidence
        if len(got) > 1:
            failures.append(f"digests: {cell}: runs disagree on digest: {sorted(got)}")
        actual = next(iter(got))
        if actual != want:
            failures.append(f"digests: {cell}: oracle mismatch: expected {want}, got {actual}")

    for scenario in sorted({scenario_of(c) for c in oracle if scenario_of(c)} - {None}):
        values = {c: cell_digests.get(c, set()) for c in cells if scenario_of(c) == scenario}
        flat = {v for vs in values.values() for v in vs}
        if len(flat) > 1:
            failures.append(
                f"digests: scenario {scenario}: cross-contender parity broken: "
                + ", ".join(f"{c}={sorted(vs)}" for c, vs in sorted(values.items()))
            )
    oracle_scenarios = {scenario_of(c) for c in oracle} - {None}
    unexpected_digest_cells = [
        c
        for c in cells
        if cell_digests[c] and scenario_of(c) not in oracle_scenarios
    ]
    if unexpected_digest_cells:
        # Not a failure: non-payload fixtures may also print a digest;
        # they are simply outside the oracle contract.
        print(
            "note: cells logging a digest outside the oracle (not asserted): "
            + ", ".join(unexpected_digest_cells)
        )

    # ---- verdict -------------------------------------------------------
    if failures:
        print(f"parity-52: {len(failures)} failure(s):")
        for f in failures:
            print(f"FAIL: {f}")
        return 1

    min_samples = min(sample_counts.values()) if sample_counts else 0
    print("parity-52: PASS")
    print(f"  cells: {len(cells)}/{EXPECTED_CELLS} sample dirs")
    print(f"  samples: min {min_samples} per cell (required >={MIN_SAMPLES})")
    print(f"  markers: 1 distinct {len(markers)}-scenario marker per evidence file, all cells")
    print(f"  digests: {len(oracle)}/{len(oracle)} oracle cells equal, cross-contender parity holds")
    print(f"  order_seed: {order_seed}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
