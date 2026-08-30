#!/usr/bin/env python3
r"""Build a deterministic run.json record + summary.md from a harness run dir.

Reads the REAL run.sh output layout — FLAT cell dirs (run.sh does
`cell_safe="${cell//\//_}"`):

    $RUN_DIR/<scenario>_<contender>/m2-summary.json   (protocol B p99 latency)
    $RUN_DIR/<scenario>_<contender>/m3-summary.json   (throughput)
    $RUN_DIR/<scenario>_<contender>/m4-summary.json   (RSS delta)
    $RUN_DIR/<scenario>_<contender>/samples.txt       (m1 cold-start raw)

m3/m4 cell identity comes from each summary JSON's own `cell` field
(slash form `scenario/contender`) — never from parsing dir names. m2
summaries (parse-protocol-b output) carry no `cell` field and m1 dirs
have no summary, so those dir names are split by longest
`scenario + "_"` prefix against the scenario set anchored by the
sibling summaries' `cell` fields (a pure m1 run falls back to meta's
`subset` field). A run dir that resolves to 0 cells is a LOUD error —
never a silent empty record.

There is no m1 summary JSON: m1 medians are computed here from the raw
samples. Ratio math is delegated to `bench-loadgen aggregate-ratios
--json` (single bootstrap implementation; this module does NO bootstrap
math, and normalizes the binary's cell-dir-basename
numerator/denominator vocabulary to the bare contender names pinned in
SCHEMA.md), and canonical input digests are delegated to
`bench-loadgen payload-digest` (both shell-outs are loud on failure,
with the command prefix overridable via BENCH_AGGREGATE_RATIOS_BIN /
BENCH_PAYLOAD_DIGEST_BIN). A scenario without a canonical payload
contract records `input_sha256: null` (see `input_sha256`).

Three modes (see `main`):

- summarize (default): `--run-dir <raw> --meta <json> --out-dir <dir>`
  builds run.json + summary.md into out-dir.
- publish: `--publish --run-dir <summarized>` copies a validated
  record dir (must contain run.json) into records/<run_id>/ and
  rebuilds records/index.json (SCHEMA.md object shape, date
  ascending, same-date ties by run_id sequence). A duplicate run_id
  with different content is refused; identical content is a no-op
  success.
- check: `--check <records-dir>` cross-checks index.json against the
  run dirs (every run dir must have an index entry, every entry must
  resolve to a run dir, and each entry's date/era/git_commit must
  match its run.json — any mismatch exits 1 naming the offender), then
  guards the digest-pinned runner: any record whose `container_digest`
  is not a pinned digest reference (`sha256:<64hex>` or
  `<repo>@sha256:<64hex>`), is not a string or null (type hole), or is
  empty/null while `host_provenance.containerized` is true, exits 1
  naming the record. Finally it regenerates every summary.md from its
  sibling run.json and diffs — any mismatch (hand-edited summary)
  exits 1 naming the file. A nonexistent records dir is an error
  (exit 1), never a silent green. Zero run.json files with an empty
  index is a green no-op.

Contract: benchmarks/records/SCHEMA.md (run.json v1, index.json v1).
Stdlib only.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import shutil
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path

SCHEMA_VERSION = 1
INDEX_SCHEMA_VERSION = 1

# benchmarks/harness/summarize.py -> benchmarks/records
DEFAULT_RECORDS_DIR = Path(__file__).resolve().parent.parent / "records"

# metric -> (summary filename, round-values key, unit)
_SUMMARY_METRICS = {
    "m2": ("m2-summary.json", "round_p99s_ns", "ns"),
    "m3": ("m3-summary.json", "per_round_means", "msgs/s"),
    "m4": ("m4-summary.json", "delta_distribution", "KiB"),
}

_RUN_DIR_TS = re.compile(r"^(\d{4})(\d{2})(\d{2})T")

DEFAULT_AGGREGATE_RATIOS = (
    "cargo run -p bench-loadgen --bin bench-loadgen -- aggregate-ratios"
)
DEFAULT_PAYLOAD_DIGEST = (
    "cargo run -p bench-loadgen --bin bench-loadgen -- payload-digest"
)

# Ratio pairing rule (pinned in SCHEMA.md): the numerator contender is
# `rust-camel-lib` whenever the scenario measured it, else the
# alphabetically first contender.
PREFERRED_NUMERATOR = "rust-camel-lib"


def _warn(msg):
    print(f"warning: {msg}", file=sys.stderr)


def round_values(values, where):
    """Coerce summary round values to floats, preserving order.

    Non-numeric entries are skipped with a stderr warning naming the
    cell context (`where`) — never silently dropped, never coerced to
    a poisoning 0.0 in the median.
    """
    out = []
    for v in values:
        try:
            out.append(float(v))
        except (TypeError, ValueError):
            _warn(f"{where}: skipping non-numeric round value {v!r}")
    return out


def input_sha256(scenario, payload_class):
    """Canonical input digest via `bench-loadgen payload-digest`.

    The binary spec is env-overridable (BENCH_PAYLOAD_DIGEST_BIN, same
    shape as BENCH_AGGREGATE_RATIOS_BIN). A non-zero exit whose stderr
    contains `unknown scenario` means the scenario has no canonical
    payload contract (its measurement input is not a canonical body):
    returns None — recorded as `input_sha256: null` — after a stderr
    warning. Every OTHER failure (binary missing, any other error) is
    LOUD: a RuntimeError naming scenario/class propagates — there is
    deliberately NO fallback to hashing the on-disk summary artifact,
    which would silently fake a canonical input digest.
    """
    spec = os.environ.get("BENCH_PAYLOAD_DIGEST_BIN", DEFAULT_PAYLOAD_DIGEST)
    # Both `--key=value` and `--key value` are accepted by the
    # binary's parser (cli.rs `parse_flags`); use the canonical
    # documented `--key=value` form.
    argv = shlex.split(spec) + [
        f"--scenario={scenario}", f"--payload-class={payload_class}",
    ]
    try:
        proc = subprocess.run(argv, capture_output=True, text=True)
    except OSError as e:
        raise RuntimeError(
            f"payload-digest unavailable for {scenario}/{payload_class}: {e}"
        ) from e
    if proc.returncode != 0:
        if "unknown scenario" in proc.stderr:
            _warn(
                "input_sha256: no canonical payload contract "
                f"for scenario {scenario}"
            )
            return None
        raise RuntimeError(
            f"payload-digest failed for {scenario}/{payload_class} "
            f"(rc={proc.returncode}): {proc.stderr.strip()}"
        )
    digest = proc.stdout.strip()
    if not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise RuntimeError(
            f"payload-digest for {scenario}/{payload_class} printed "
            f"unexpected output: {digest!r} (want one lowercase hex line)"
        )
    return f"sha256:{digest}"


def _cached_input_sha256(cache, scenario, payload_class):
    """Memoized [`input_sha256`] — one payload-digest subprocess per
    (scenario, payload-class) pair, not per cell (every cell of a
    scenario shares the same canonical input, so the first cell pays
    the subprocess and the rest reuse the result)."""
    key = (scenario, payload_class)
    if key not in cache:
        cache[key] = input_sha256(scenario, payload_class)
    return cache[key]


def _m1_samples(path):
    """Startup-ms values from a measure_once samples.txt.

    Each measured line is `<elapsed_ms> <rss_kb>` (run.sh measure_once).
    Any line whose first token is not numeric (headers, warnings) is
    skipped explicitly; the median is computed by the caller.
    """
    values = []
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        tokens = line.split()
        if not tokens:
            continue
        try:
            values.append(float(tokens[0]))
        except ValueError:
            continue  # non-numeric line: skip explicitly
    return values


def _split_flat_dir(name, scenarios):
    """(scenario, contender) for a flat `<scenario>_<contender>` dir
    name, split by longest `scenario + "_"` prefix so contenders
    containing `_` and scenario names are never confused. No match is
    a loud ValueError, never a guess."""
    matches = [s for s in scenarios if name.startswith(s + "_")]
    if not matches:
        raise ValueError(
            f"flat cell dir {name!r}: no scenario prefix match "
            f"(known scenarios: {sorted(scenarios)})"
        )
    scenario = max(matches, key=len)
    return scenario, name[len(scenario) + 1:]


def load_cells(run_dir, meta=None):
    """Per-cell measurements from a run dir (unsorted list of cell dicts).

    The REAL run.sh layout is FLAT: every metric artifact lives in a
    `<scenario>_<contender>/` cell dir at the run root. m3/m4 cell
    identity comes from each summary JSON's `cell` field (slash form)
    — never from parsing dir names; those `cell` fields also anchor
    the scenario set used to split the m2 and m1 dir names (m2
    summaries carry no `cell` field, m1 dirs have no summary). A pure
    m1 run (no summaries anywhere) resolves its scenarios from meta's
    `subset` field (comma-joined scenario names, the index.json
    vocabulary); an unresolvable dir is a loud ValueError. Cells whose
    summary reports failure (`status != "ok"`, or m2
    `is_invalidated`) are skipped — no medians over partial data; a
    cell with empty/all-malformed round values is skipped with a
    stderr warning naming it. A run dir that resolves to 0 cells
    raises naming the run dir — never a silent empty record. Canonical
    input digests are memoized per (scenario, payload_class) within
    this call.
    """
    run_dir = Path(run_dir)
    dirs = sorted(p for p in run_dir.iterdir() if p.is_dir())
    # Pass 1: parse every summary once; m3/m4 `cell` fields anchor the
    # scenario vocabulary for the m1/m2 dir-name splits.
    parsed = {}  # dir name -> {metric: summary object}
    scenarios = set()
    for entry in dirs:
        for metric, (fname, _, _) in sorted(_SUMMARY_METRICS.items()):
            path = entry / fname
            if not path.is_file():
                continue
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
            except (json.JSONDecodeError, UnicodeDecodeError):
                _warn(f"{entry.name}/{fname}: unparseable summary; skipping")
                continue
            parsed.setdefault(entry.name, {})[metric] = data
            if metric in ("m3", "m4"):
                cell = data.get("cell")
                if isinstance(cell, str):
                    scenario = cell.partition("/")[0]
                    if scenario:
                        scenarios.add(scenario)
    if not scenarios and meta is not None and meta.get("subset"):
        # Pure m1 run: scenario vocabulary from meta's `subset`.
        scenarios = {
            s.strip() for s in str(meta["subset"]).split(",") if s.strip()
        }
    # Pass 2: emit cells (identity from `cell` fields; m1/m2 via
    # longest-prefix split).
    digest_cache = {}
    cells = []
    for entry in dirs:
        for metric, data in sorted(parsed.get(entry.name, {}).items()):
            cell = _summary_cell(entry, metric, data, scenarios, digest_cache)
            if cell is not None:
                cells.append(cell)
        if (entry / "samples.txt").is_file():
            cell = _m1_cell(entry, scenarios, digest_cache)
            if cell is not None:
                cells.append(cell)
    if not cells:
        raise ValueError(
            f"run dir {run_dir}: resolved 0 measurement cells (no "
            "ok-status m2/m3/m4 summaries with usable values, no "
            "numeric m1 samples) — refusing to emit an empty record"
        )
    return cells


def _summary_cell(entry, metric, data, scenarios, digest_cache):
    """One m2/m3/m4 cell dict from a parsed summary, or None.

    m3/m4 identity comes from the summary's `cell` field; m2
    (parse-protocol-b output) has none, so its dir name is split
    against the run's scenario set. Failure statuses and empty round
    values skip the cell (the latter with a warning); the 0-cell
    guard in [`load_cells`] catches total loss.
    """
    fname, values_key, unit = _SUMMARY_METRICS[metric]
    cell_field = data.get("cell")
    if metric in ("m3", "m4"):
        if not (isinstance(cell_field, str) and "/" in cell_field):
            _warn(
                f"{entry.name}/{fname}: missing or malformed `cell` "
                f"field ({cell_field!r}); skipping"
            )
            return None
        scenario, _, contender = cell_field.partition("/")
        if not scenario or not contender:
            _warn(
                f"{entry.name}/{fname}: malformed `cell` field "
                f"{cell_field!r}; skipping"
            )
            return None
    else:  # m2: no `cell` field in parse-protocol-b output
        scenario, contender = _split_flat_dir(entry.name, scenarios)
    cell_id = f"{scenario}/{contender}/{metric}"
    if data.get("status", "ok") != "ok":
        return None
    if data.get("is_invalidated", False):
        return None
    raw = data.get(values_key)
    values = round_values(raw, cell_id) if isinstance(raw, list) else []
    if not values:
        _warn(f"cell {cell_id}: empty or all-malformed values; skipping")
        return None
    return {
        "scenario": scenario,
        "contender": contender,
        "variant": "default",
        "payload_class": "shared",
        "metric": metric,
        "round_values": values,
        "median": float(statistics.median(values)),
        "unit": unit,
        "input_sha256": _cached_input_sha256(digest_cache, scenario, "shared"),
    }


def _m1_cell(entry, scenarios, digest_cache):
    """m1 cell from `<scenario>_<contender>/samples.txt`.

    The scenario vocabulary comes from sibling summaries' `cell`
    fields (or meta's `subset` for pure m1 runs — see
    [`load_cells`]); the dir name is split by longest prefix so
    contenders containing `_` are never confused with scenario names.
    No match: loud ValueError.
    """
    scenario, contender = _split_flat_dir(entry.name, scenarios)
    values = _m1_samples(entry / "samples.txt")
    if not values:
        _warn(f"cell {scenario}/{contender}/m1: no numeric samples; skipping")
        return None
    return {
        "scenario": scenario,
        "contender": contender,
        "variant": "default",
        "payload_class": "shared",
        "metric": "m1",
        "round_values": values,
        "median": float(statistics.median(values)),
        "unit": "ms",
        "input_sha256": _cached_input_sha256(digest_cache, scenario, "shared"),
    }


def _run_date(run_dir, meta):
    """(ISO date, YYYYMMDD) derived from the run dir name, meta fallback."""
    match = _RUN_DIR_TS.match(Path(run_dir).name)
    if match:
        y, m, d = match.groups()
        return f"{y}-{m}-{d}", f"{y}{m}{d}"
    iso = meta.get("date")
    if iso:
        compact = iso[:10].replace("-", "")
        return iso[:10], compact
    raise ValueError(
        f"cannot derive run date from run dir {Path(run_dir).name!r}; "
        "set meta.date (ISO-8601)"
    )


def build_record(run_dir, meta):
    """Assemble the run.json object per benchmarks/records/SCHEMA.md.

    `meta` supplies git_commit, container_digest, era, protocol,
    host_provenance (plus optional date). `run_id` (or `run_seq` to
    compose `<YYYYMMDD>-v<N>`) is REQUIRED — there is no default
    sequence fallback. Ratios are NOT computed here — main() fills
    them via compute_ratios so the builder stays subprocess-free and
    deterministic.
    """
    date, compact = _run_date(run_dir, meta)
    run_id = meta.get("run_id")
    if run_id is None:
        run_seq = meta.get("run_seq")
        if run_seq is None:
            raise ValueError(
                "meta must set run_id (or run_seq to compose "
                "<YYYYMMDD>-v<N>); no default sequence fallback"
            )
        run_id = f"{compact}-v{run_seq}"
    cells = sorted(
        load_cells(run_dir, meta),
        key=lambda c: (
            c["scenario"],
            c["contender"],
            c["variant"],
            c["payload_class"],
            c["metric"],
        ),
    )
    return {
        "schema_version": SCHEMA_VERSION,
        "run_id": run_id,
        "era": str(meta["era"]),
        "date": date,
        "git_commit": meta["git_commit"],
        "container_digest": meta.get("container_digest"),
        "host_provenance": meta["host_provenance"],
        "protocol": meta["protocol"],
        "cells": cells,
        "ratios": [],
    }


def _aggregate_ratios_argv():
    spec = os.environ.get("BENCH_AGGREGATE_RATIOS_BIN", DEFAULT_AGGREGATE_RATIOS)
    return shlex.split(spec)


def _ratio_row(row, scenario, numerator, denominator):
    """One aggregate-ratios JSON row normalized to SCHEMA vocabulary.

    The live binary reports numerator/denominator as CELL-DIR
    basenames (`<scenario>_<contender>`, e.g.
    `http-server_rust-camel-lib`); run.json's `ratios` speak bare
    contender names (SCHEMA.md). The names are overwritten with the
    contenders chosen here, and if the binary reported the pair in
    the opposite direction the values are mirrored with them —
    ratio(A,B) = 1/ratio(B,A), and the CI bounds swap under
    inversion. Values are guaranteed positive: the binary rejects
    non-positive per-round means before any math runs. All other
    fields (e.g. `method`) pass through verbatim.
    """
    num_dir = f"{scenario}_{numerator}"
    den_dir = f"{scenario}_{denominator}"
    got = str(row.get("numerator", ""))
    out = dict(row)
    if got == den_dir:
        out["point"] = 1.0 / row["point"]
        out["ci_lo"] = 1.0 / row["ci_hi"]
        out["ci_hi"] = 1.0 / row["ci_lo"]
    elif got != num_dir:
        raise RuntimeError(
            f"aggregate-ratios returned unexpected numerator {got!r}; "
            f"expected {num_dir!r} or {den_dir!r}"
        )
    out["numerator"] = numerator
    out["denominator"] = denominator
    return out


def compute_ratios(record, run_dir=None):
    """Ratio rows via `bench-loadgen aggregate-ratios --json` (no local math).

    Per scenario, the numerator is `rust-camel-lib` when that contender
    was measured (pairing rule pinned in SCHEMA.md), else the
    alphabetically first contender; it is paired against each remaining
    contender in alphabetical order. The binary receives the FLAT cell
    dirs (`<run_dir>/<scenario>_<contender>/m3-summary.json`); it
    needs `measurement_order.json` at the run root (provenance
    validation) and prints a single JSON object per invocation whose
    numerator/denominator are cell-dir basenames — [`_ratio_row`]
    normalizes those to the bare contender names. Rows are sorted by
    (numerator, denominator, metric).
    """
    if run_dir is None:
        raise ValueError("compute_ratios needs run_dir to locate m3-summary.json")
    base_argv = _aggregate_ratios_argv()
    cells = [c for c in record["cells"] if c["metric"] == "m3"]
    ratios = []
    for scenario in sorted({c["scenario"] for c in cells}):
        contenders = sorted(
            c["contender"] for c in cells if c["scenario"] == scenario
        )
        numerator = (
            PREFERRED_NUMERATOR
            if PREFERRED_NUMERATOR in contenders
            else contenders[0]
        )
        for denominator in contenders:
            if denominator == numerator:
                continue
            argv = base_argv + [
                str(Path(run_dir) / f"{scenario}_{numerator}" / "m3-summary.json"),
                str(Path(run_dir) / f"{scenario}_{denominator}" / "m3-summary.json"),
                "--json",
            ]
            proc = subprocess.run(
                argv, capture_output=True, text=True, check=True
            )
            parsed = json.loads(proc.stdout)
            for row in (parsed if isinstance(parsed, list) else [parsed]):
                ratios.append(
                    _ratio_row(row, scenario, numerator, denominator)
                )
    ratios.sort(key=lambda r: (r["numerator"], r["denominator"], r["metric"]))
    return ratios


def _dump(record):
    return json.dumps(record, sort_keys=True, indent=2) + "\n"


def emit_json(record, out_dir):
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / "run.json"
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(_dump(record))
    return path


def _fmt(value):
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return repr(float(value))
    return str(value)


def emit_summary(record, out_dir):
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    lines = [
        f"# Run {record['run_id']}",
        "",
        f"- date: {record['date']}",
        f"- era: {record['era']}",
        f"- git_commit: {record['git_commit']}",
        "",
    ]
    by_metric = {}
    for cell in record["cells"]:
        by_metric.setdefault(cell["metric"], []).append(cell)
    for metric in sorted(by_metric):
        rows = sorted(
            by_metric[metric],
            key=lambda c: (c["scenario"], c["contender"]),
        )
        lines.append(f"## Metric {metric} ({rows[0]['unit']})")
        lines.append("")
        lines.append("| scenario | contender | median |")
        lines.append("| --- | --- | --- |")
        for c in rows:
            lines.append(
                f"| {c['scenario']} | {c['contender']} | {_fmt(c['median'])} |"
            )
        lines.append("")
    if record["ratios"]:
        lines.append("## Ratios")
        lines.append("")
        lines.append(
            "| numerator | denominator | metric | point | ci_lo | ci_hi"
            " | method |"
        )
        lines.append("| --- | --- | --- | --- | --- | --- | --- |")
        for r in record["ratios"]:
            lines.append(
                "| {numerator} | {denominator} | {metric} | {point}"
                " | {ci_lo} | {ci_hi} | {method} |".format(
                    numerator=r["numerator"],
                    denominator=r["denominator"],
                    metric=r["metric"],
                    point=_fmt(r["point"]),
                    ci_lo=_fmt(r["ci_lo"]),
                    ci_hi=_fmt(r["ci_hi"]),
                    method=r["method"],
                )
            )
        lines.append("")
    path = out_dir / "summary.md"
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines))
    return path


def _run_id_seq(run_id):
    """Numeric sequence of a `<YYYYMMDD>-v<N>` run_id, -1 otherwise.

    Sorting same-date ties by this sequence (v2 before v10) is what
    plain lexicographic order would invert.
    """
    match = re.fullmatch(r"\d{8}-v(\d+)", str(run_id))
    return int(match.group(1)) if match else -1


def sort_index_entries(runs):
    """SCHEMA.md index order: date ascending, same-date ties by
    run_id sequence (then run_id for total determinism)."""
    return sorted(
        runs,
        key=lambda e: (
            e["date"],
            _run_id_seq(e.get("run_id", "")),
            e.get("run_id", ""),
        ),
    )


def load_index(records_dir):
    """Load records/index.json; seed the v1 object when absent."""
    path = Path(records_dir) / "index.json"
    if not path.is_file():
        return {"index_schema_version": INDEX_SCHEMA_VERSION, "runs": []}
    index = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(index, dict) or not isinstance(index.get("runs"), list):
        raise ValueError(
            f"{path}: index must be an object with a 'runs' array "
            f"(index_schema_version {INDEX_SCHEMA_VERSION})"
        )
    return index


def index_entry(record):
    """Index entry for a run.json object (SCHEMA.md `index.json`).

    `subset` is derived from the cells' scenario names (comma-joined,
    sorted) per the SCHEMA.md subset vocabulary; `path` is relative
    to records/ and points at the run DIRECTORY.
    """
    scenarios = sorted({c["scenario"] for c in record.get("cells", [])})
    return {
        "run_id": record["run_id"],
        "date": record["date"],
        "era": str(record["era"]),
        "git_commit": record["git_commit"],
        "subset": ",".join(scenarios),
        "path": f"{record['run_id']}/",
    }


def _dir_snapshot(path):
    """{relative name: bytes} for every file under path (recursive)."""
    path = Path(path)
    return {
        str(p.relative_to(path)): p.read_bytes()
        for p in sorted(path.rglob("*"))
        if p.is_file()
    }


def publish_run(record_dir, records_dir):
    """Copy a summarized record dir into records/ and rebuild the index.

    The source must be a VALIDATED run dir (contains run.json with the
    SCHEMA.md identity fields). A duplicate run_id with different
    content is refused (exit 2); identical content is a no-op success
    (exit 0) that also reconciles a missing index entry. Returns the
    process exit code.
    """
    record_dir = Path(record_dir)
    records_dir = Path(records_dir)
    run_json = record_dir / "run.json"
    if not run_json.is_file():
        print(
            f"error: {run_json}: not a summarized run dir (no run.json); "
            "run summarize mode first",
            file=sys.stderr,
        )
        return 2
    try:
        record = json.loads(run_json.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        print(f"error: {run_json}: invalid JSON ({e})", file=sys.stderr)
        return 2
    for key in ("run_id", "date", "era", "git_commit"):
        if key not in record:
            print(f"error: {run_json}: missing {key!r}", file=sys.stderr)
            return 2
    run_id = record["run_id"]
    dest = records_dir / run_id
    if dest.exists():
        if _dir_snapshot(dest) != _dir_snapshot(record_dir):
            print(
                f"error: refusing to publish run {run_id}: {dest} exists "
                "with different content",
                file=sys.stderr,
            )
            return 2
        # Identical content: no-op success, but reconcile a missing
        # index entry (hand-deleted or drifted) so the index stays
        # authoritative over the run dirs.
        index = load_index(records_dir)
        if any(r.get("run_id") == run_id for r in index["runs"]):
            print(f"already published, identical: {dest}")
            return 0
        entry = index_entry(record)
        runs = [r for r in index["runs"] if r.get("run_id") != run_id]
        runs.append(entry)
        index["runs"] = sort_index_entries(runs)
        with open(
            records_dir / "index.json", "w", encoding="utf-8", newline="\n"
        ) as f:
            f.write(_dump(index))
        print(f"already published, identical: {dest}")
        print(f"reconciled missing index entry for {run_id}")
        return 0
    records_dir.mkdir(parents=True, exist_ok=True)
    shutil.copytree(record_dir, dest)
    index = load_index(records_dir)
    entry = index_entry(record)
    runs = [r for r in index["runs"] if r.get("run_id") != run_id]
    runs.append(entry)
    index["runs"] = sort_index_entries(runs)
    with open(
        records_dir / "index.json", "w", encoding="utf-8", newline="\n"
    ) as f:
        f.write(_dump(index))
    print(f"published {dest}")
    print(f"updated {records_dir / 'index.json'}")
    return 0


def _index_dir_orphans(records_dir, run_dirs, index):
    """Orphan descriptions from the index<->run-dirs cross-check.

    Every run dir (contains run.json) must have an index entry whose
    path matches it, and every index entry's path must resolve to a
    run dir. Returns a list of human-readable orphan descriptions
    (empty when the two sides are consistent).
    """
    index_paths = {str(e.get("path", "")).rstrip("/") for e in index["runs"]}
    orphans = []
    for run_dir in run_dirs:
        if run_dir.name not in index_paths:
            orphans.append(f"run dir {run_dir.name}/ has no index entry")
    dir_names = {d.name for d in run_dirs}
    for entry in index["runs"]:
        name = str(entry.get("path", "")).rstrip("/")
        if name not in dir_names:
            orphans.append(f"index entry {name!r} has no run dir")
    return orphans


def check_records(records_dir):
    """Verify records/ consistency: index<->dirs, digest pinning,
    index<->run.json identity, and summary<->run.json.

Guards, all exit 1 naming the offender on failure:
    1. A nonexistent records dir is an error, never a silent green.
    2. Cross-check index.json against the run dirs — every run dir
       (contains run.json) must have an index entry with a matching
       path, and every index entry's path must resolve to a run dir.
    3. Digest-pinned runner: a record whose `container_digest` is
       not a pinned digest reference (`sha256:<64hex>` or
       `<repo>@sha256:<64hex>`), is not a string or null (type
       hole), or is empty/null while
       `host_provenance.containerized` is true, is rejected —
       mutable tags, malformed references, malformed types, and
       digest-less containerized runs never pass the guard.
    4. Each index entry's date/era/git_commit must equal the values
       in the entry's run.json.
    5. Regenerate every summary.md from its sibling run.json and
       byte-diff (hand-edited summaries are forbidden).
    A records dir with zero run.json files and an empty index is a
    green no-op (exit 0). Returns the process exit code.
    """
    records_dir = Path(records_dir)
    if not records_dir.is_dir():
        print(f"error: records dir not found: {records_dir}", file=sys.stderr)
        return 1
    run_dirs = sorted(
        p for p in records_dir.iterdir()
        if p.is_dir() and (p / "run.json").is_file()
    )
    index = load_index(records_dir)
    orphans = _index_dir_orphans(records_dir, run_dirs, index)
    if orphans:
        for name in orphans:
            print(f"error: index/dir mismatch: {name}", file=sys.stderr)
        return 1
    if not run_dirs:
        print("check: no published run.json files; nothing to verify")
        return 0
    index_by_path = {
        str(e.get("path", "")).rstrip("/"): e for e in index["runs"]
    }
    problems = []
    with tempfile.TemporaryDirectory() as tmp:
        for run_dir in run_dirs:
            try:
                record = json.loads(
                    (run_dir / "run.json").read_text(encoding="utf-8")
                )
            except Exception as e:
                problems.append(
                    f"{run_dir}/run.json: cannot parse record ({e})"
                )
                continue
            name = str(record.get("run_id") or run_dir.name)
            # Identity: the index entry must agree with run.json.
            entry = index_by_path.get(run_dir.name, {})
            for field in ("date", "era", "git_commit"):
                if entry.get(field) != record.get(field):
                    problems.append(
                        f"index entry {name}: {field} "
                        f"{entry.get(field)!r} != run.json "
                        f"{record.get(field)!r}"
                    )
            # Digest-pinned runner (mutable tags and malformed
            # references forbidden; the field itself must be a string
            # or null — any other JSON type is a malformed record,
            # not an acceptable digest).
            digest = record.get("container_digest")
            containerized = bool(
                (record.get("host_provenance") or {}).get("containerized")
            )
            if digest is not None and not isinstance(digest, str):
                problems.append(
                    f"record {name}: container_digest must be a string "
                    f"or null, got {type(digest).__name__}"
                )
            elif isinstance(digest, str) and not re.fullmatch(
                r"(?:sha256:[0-9a-f]{64}|[^@]+@sha256:[0-9a-f]{64})",
                digest,
            ):
                problems.append(
                    f"record {name}: container_digest is a mutable tag "
                    f"or malformed reference ({digest!r}); record the "
                    "pinned sha256 digest"
                )
            elif containerized and not digest:
                problems.append(
                    f"record {name}: containerized run has an empty "
                    "container_digest; record the pinned sha256 digest"
                )
            try:
                out = Path(tmp) / run_dir.name
                emit_summary(record, out)
                generated = (out / "summary.md").read_bytes()
            except Exception as e:
                problems.append(
                    f"{run_dir}/run.json: cannot regenerate summary ({e})"
                )
                continue
            published = run_dir / "summary.md"
            if not published.is_file() or published.read_bytes() != generated:
                problems.append(
                    "summary does not match run.json "
                    f"(hand-edited?): {published}"
                )
    if problems:
        for name in problems:
            print(f"error: {name}", file=sys.stderr)
        return 1
    print(f"check: {len(run_dirs)} summaries match their run.json")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(
        description=(
            "Build run.json + summary.md from a harness run dir; "
            "publish records and guard generated summaries."
        )
    )
    parser.add_argument(
        "--run-dir",
        help="run output directory (summarize) or summarized record "
        "dir containing run.json (publish)",
    )
    parser.add_argument("--meta", help="meta JSON path (summarize mode)")
    parser.add_argument(
        "--out-dir", help="record output directory (summarize mode)"
    )
    parser.add_argument(
        "--publish",
        action="store_true",
        help="publish a summarized record dir into --records-dir",
    )
    parser.add_argument(
        "--records-dir",
        type=Path,
        default=DEFAULT_RECORDS_DIR,
        help="records directory for --publish "
        f"(default: {DEFAULT_RECORDS_DIR})",
    )
    parser.add_argument(
        "--check",
        type=Path,
        metavar="RECORDS_DIR",
        help="verify every summary.md regenerates from its run.json",
    )
    args = parser.parse_args(argv)

    if args.check is not None:
        return check_records(args.check)
    if args.publish:
        if args.run_dir is None:
            parser.error(
                "--publish requires --run-dir (summarized record dir)"
            )
        return publish_run(args.run_dir, args.records_dir)
    if args.run_dir is None or args.meta is None or args.out_dir is None:
        parser.error(
            "summarize mode requires --run-dir, --meta and --out-dir "
            "(or use --publish / --check)"
        )

    try:
        meta = json.loads(Path(args.meta).read_text(encoding="utf-8"))
        record = build_record(args.run_dir, meta)
        record["ratios"] = compute_ratios(record, run_dir=args.run_dir)
    except (ValueError, RuntimeError, OSError, subprocess.CalledProcessError) as e:
        # Loud, non-zero, no empty record emitted (0-cell run dirs,
        # unresolvable cell dirs, bad meta, delegation-binary
        # failures) — a traceback adds nothing.
        print(f"error: {e}", file=sys.stderr)
        return 1
    run_json = emit_json(record, args.out_dir)
    summary_md = emit_summary(record, args.out_dir)
    print(f"wrote {run_json}")
    print(f"wrote {summary_md}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
