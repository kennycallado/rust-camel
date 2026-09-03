"""Tests for publish/check modes — stdlib unittest, synthetic fixtures.

Publish/check operate on SUMMARIZED record dirs (run.json +
summary.md), so fixtures here are hand-built records emitted through
`emit_json`/`emit_summary` — no payload-digest subprocesses, unlike
test_summarize.py which exercises the raw run-dir pipeline.
"""
import contextlib
import io
import json
import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import summarize

COMMIT = "a" * 40

# Registered contender vocabulary (mirrors run.sh
# SCENARIO_ARTIFACT_SET): full scenarios measure 8 contenders; bridge
# scenarios (xsd-validation-bridge, xslt-bridge) measure 6 — core 4 +
# node 2 (YAML variants carry no bridge-tax signal).
FULL_CONTENDERS = (
    "camel-quarkus-dsl-native", "camel-quarkus-yaml-native",
    "camel-standalone-dsl", "camel-standalone-yaml",
    "node-fastify", "node-native", "rust-camel-cli", "rust-camel-lib",
)
BRIDGE_CONTENDERS = (
    "camel-quarkus-dsl-native", "camel-standalone-dsl",
    "node-fastify", "node-native", "rust-camel-cli", "rust-camel-lib",
)

_UNITS = {"m1": "ms", "m2": "ns", "m3": "msgs/s", "m4": "KiB"}


def _cell(scenario="startup-minimal", contender="rust-camel-lib",
          metric="m1"):
    return {
        "scenario": scenario,
        "contender": contender,
        "variant": "default",
        "payload_class": "shared",
        "metric": metric,
        "round_values": [10.0, 12.0],
        "median": 11.0,
        "unit": _UNITS[metric],
        "input_sha256": None,
    }


def _metrics_cells(scenario, contenders, metrics=("m1", "m2")):
    """One cell dict per (contender, metric) — a complete roster's
    worth of cells when contenders is the full set."""
    return [
        _cell(scenario=scenario, contender=c, metric=m)
        for c in contenders
        for m in metrics
    ]


_REASON = "warmup failed-stability: MessageBoundUnconverged"


def _attempted_cell(scenario="split-aggregate", contender="rust-camel-lib",
                    status="unconverged", reason=_REASON, rounds=2):
    """ATTEMPTED m2 cell (schema_version 2): status/reason/rounds, NO
    latency fields — the shape summarize.py emits from classified
    attempt evidence."""
    return {
        "scenario": scenario,
        "contender": contender,
        "metric": "m2",
        "status": status,
        "reason": reason,
        "rounds": rounds,
    }


def _split_roster():
    return [f"split-aggregate/{c}" for c in FULL_CONTENDERS]


def _split_cells_with_attempt(attempt_cell):
    """Complete split-aggregate roster whose rust-camel-lib m2 cell is
    `attempt_cell`; every other cell is measured m1+m2."""
    others = [c for c in FULL_CONTENDERS if c != "rust-camel-lib"]
    return (
        _metrics_cells("split-aggregate", others)
        + _metrics_cells("split-aggregate", ["rust-camel-lib"],
                         metrics=("m1",))
        + [attempt_cell]
    )


# Attempt-evidence sentinels, verbatim harness-written lines (the
# artifact format is the contract — mirrors test_summarize.py).
UNCONVERGED_EVIDENCE = (
    "measure-a: error: warmup failed-stability: "
    "MessageBoundUnconverged\n"
    "status=failed reason=measure-a-error\n"
)
PROBE_TIMEOUT_EVIDENCE = (
    "# probe reason: no BENCH_LATENCY within 30s timeout\n"
)

# meta.json for the e2e gap-family fixture: startup-minimal (cold-only,
# warm n/a) + xslt-bridge (bridge roster, warm-applicable).
E2E_META = {
    "era": "2",
    "git_commit": COMMIT,
    "container_digest": None,
    "run_id": "20260906T060000Z",
    "scenarios": "startup-minimal,xslt-bridge",
    "protocol": {
        "rounds": 2,
        "duration_secs": 10.0,
        "warmup_secs": 2.0,
        "order_seed": 0,
    },
    "host_provenance": {
        "cpu_model": "test-cpu",
        "cores": 8,
        "kernel": "test-kernel",
        "containerized": False,
        "load": {
            "one": 0.1,
            "five": 0.2,
            "fifteen": 0.3,
        },
    },
}


def make_e2e_gap_families_run(root, run_id="20260906T060000Z",
                              with_timeout_evidence=True):
    """Synthetic run dir for the canonical gap-family e2e (Task 3.1).

    m1 evidence for EVERY roster identity: startup-minimal (full
    roster, 8 contenders) + xslt-bridge (bridge roster, 6). xslt-bridge
    m2: 4 measured cells (nested m2-summary.json), 1 unconverged cell
    (nested protocol-a-summary.txt with both sentinel lines), 1
    attempted-timeout cell (nested exit-codes.txt). With
    `with_timeout_evidence=False` the exit-codes.txt files are omitted
    — the timeout cell becomes evidence-less (no summary, no status).
    """
    run = root / run_id
    for contender in FULL_CONTENDERS:
        cell = run / f"startup-minimal_{contender}"
        cell.mkdir(parents=True)
        (cell / "samples.txt").write_text(
            "startup-ms rss-kb\n12 900\n14 950\n", encoding="utf-8"
        )
    measured = (
        "camel-quarkus-dsl-native", "camel-standalone-dsl",
        "node-fastify", "node-native",
    )
    for contender in BRIDGE_CONTENDERS:
        cell = run / f"xslt-bridge_{contender}"
        cell.mkdir(parents=True)
        (cell / "samples.txt").write_text(
            "startup-ms rss-kb\n20 900\n22 950\n", encoding="utf-8"
        )
    for rnd in ("0", "1"):
        for contender in measured:
            d = run / f"m2-round-{rnd}" / "xslt-bridge" / contender
            d.mkdir(parents=True)
            (d / "m2-summary.json").write_text(
                json.dumps({
                    "median_p99_ns": 400,
                    "round_p99s_ns": [400],
                    "total_samples": 700,
                    "malformed_records": 0,
                    "is_invalidated": False,
                }),
                encoding="utf-8",
            )
        unconv = run / f"m2-round-{rnd}" / "xslt-bridge" / "rust-camel-lib"
        unconv.mkdir(parents=True)
        (unconv / "protocol-a-summary.txt").write_text(
            UNCONVERGED_EVIDENCE, encoding="utf-8"
        )
        timeout = run / f"m2-round-{rnd}" / "xslt-bridge" / "rust-camel-cli"
        timeout.mkdir(parents=True)
        if with_timeout_evidence:
            (timeout / "exit-codes.txt").write_text(
                PROBE_TIMEOUT_EVIDENCE, encoding="utf-8"
            )
    return run


def _unknown_scenario_digest_stub(root):
    """payload-digest stub: every scenario exits 2 with `unknown
    scenario` on stderr — the real binary's answer for scenarios
    without a canonical payload contract (startup-minimal,
    xslt-bridge) — so input_sha256 records null."""
    stub = root / "stub-payload-digest.sh"
    stub.write_text(
        "#!/bin/sh\n"
        'echo "payload-digest: unknown scenario for this fixture" >&2\n'
        "exit 2\n",
        encoding="utf-8",
    )
    stub.chmod(
        stub.stat().st_mode
        | stat.S_IXUSR
        | stat.S_IXGRP
        | stat.S_IXOTH
    )
    return stub


def make_record_dir(root, run_id, date, cells=None, expected_cells=None,
                    m2_attempted_cells=None, schema_version=1):
    """Synthetic summarized record dir: run.json + generated summary.md.

    `expected_cells` defaults to the roster implied by the given cells
    (self-consistent, so legacy tests pass the completeness gate);
    m2-attempted defaults to none. Tests crafting an INCOMPLETE record
    pass an explicit roster.
    """
    if cells is None:
        cells = [_cell()]
    if expected_cells is None:
        expected_cells = sorted(
            {f"{c['scenario']}/{c['contender']}" for c in cells}
        )
    record = {
        "schema_version": schema_version,
        "run_id": run_id,
        "era": "2",
        "date": date,
        "git_commit": COMMIT,
        "container_digest": None,
        "host_provenance": {
            "cpu_model": "test-cpu",
            "cores": 1,
            "kernel": "test-kernel",
            "containerized": False,
            "load": {
                "one": 0.0,
                "five": 0.0,
                "fifteen": 0.0,
            },
        },
        "protocol": {
            "rounds": 1,
            "duration_secs": 1.0,
            "warmup_secs": 0.0,
            "order_seed": 0,
        },
        "cells": cells,
        "expected_cells": expected_cells,
        "m2_attempted_cells": m2_attempted_cells or [],
        "ratios": [],
    }
    out = root / run_id
    summarize.emit_json(record, out)
    summarize.emit_summary(record, out)
    return out


def seed_index(records_dir):
    """Phase-1 seed: the v1 index object with an empty runs array."""
    (records_dir / "index.json").write_text(
        json.dumps({"index_schema_version": 1, "runs": []}) + "\n",
        encoding="utf-8",
    )


class PublishTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.records = self.root / "records"
        self.records.mkdir()
        seed_index(self.records)

    def _publish(self, run_id, date, cells=None, expected_cells=None,
                 m2_attempted_cells=None, schema_version=1):
        source = make_record_dir(
            self.root, run_id, date, cells=cells,
            expected_cells=expected_cells,
            m2_attempted_cells=m2_attempted_cells,
            schema_version=schema_version,
        )
        err = io.StringIO()
        out = io.StringIO()
        with contextlib.redirect_stderr(err):
            with contextlib.redirect_stdout(out):
                rc = summarize.main([
                    "--publish",
                    "--run-dir", str(source),
                    "--records-dir", str(self.records),
                ])
        return rc, out.getvalue(), err.getvalue()

    def test_publish_appends_index(self):
        # Publish the LATER-dated run first: the index must come out
        # date-ascending regardless of publish order.
        rc_b, _, _ = self._publish("20260905-v6", "2026-09-05")
        rc_a, _, _ = self._publish("20260901-v5", "2026-09-01")
        self.assertEqual((rc_b, rc_a), (0, 0))
        index = json.loads(
            (self.records / "index.json").read_text(encoding="utf-8")
        )
        self.assertEqual(index["index_schema_version"], 1)
        runs = index["runs"]
        self.assertEqual(
            [r["run_id"] for r in runs],
            ["20260901-v5", "20260905-v6"],
        )
        self.assertEqual(
            [r["date"] for r in runs],
            ["2026-09-01", "2026-09-05"],
        )
        for entry in runs:
            self.assertEqual(entry["git_commit"], COMMIT)
            # path is relative to records/ and points at the run DIRECTORY
            self.assertEqual(entry["path"], f"{entry['run_id']}/")
            run_dir = self.records / entry["path"]
            self.assertTrue(run_dir.is_dir(), entry["path"])
            self.assertTrue((run_dir / "run.json").is_file())

    def test_publish_refuses_duplicate(self):
        rc, _, _ = self._publish("20260905-v6", "2026-09-05")
        self.assertEqual(rc, 0)
        # Same run_id, different content -> second publish refuses.
        rc2, _, err = self._publish(
            "20260905-v6",
            "2026-09-05",
            cells=[_cell(contender="camel-standalone-dsl")],
        )
        self.assertNotEqual(rc2, 0)
        self.assertIn("different content", err)

    def test_check_detects_hand_edit(self):
        rc, _, _ = self._publish("20260905-v6", "2026-09-05")
        self.assertEqual(rc, 0)
        summary = self.records / "20260905-v6" / "summary.md"
        with open(summary, "a", encoding="utf-8") as f:
            f.write(" x")
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = summarize.main(["--check", str(self.records)])
        self.assertEqual(rc, 1)
        self.assertIn(str(summary), err.getvalue())

    def test_check_empty_records(self):
        # Seeded index only — what the repo holds before any run
        # publishes. Green no-op.
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = summarize.main(["--check", str(self.records)])
        self.assertEqual(rc, 0)

    def test_check_missing_records_dir_exits_1(self):
        # A typo'd path must be a loud error, never a silent green
        # "nothing to verify".
        missing = self.root / "no-such-records-dir"
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = summarize.main(["--check", str(missing)])
        self.assertEqual(rc, 1)
        self.assertIn("not found", err.getvalue())
        self.assertIn(str(missing), err.getvalue())

    def test_check_index_entry_field_mismatch(self):
        # Index entries are derived views: a drifted date/era/
        # git_commit (hand-edited index) must exit 1 naming the entry.
        self._publish("20260905-v6", "2026-09-05")
        index_path = self.records / "index.json"
        index = json.loads(index_path.read_text(encoding="utf-8"))
        index["runs"][0]["date"] = "2026-09-04"
        index["runs"][0]["era"] = "1"
        index_path.write_text(
            json.dumps(index) + "\n", encoding="utf-8"
        )
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = summarize.main(["--check", str(self.records)])
        self.assertEqual(rc, 1)
        self.assertIn("20260905-v6", err.getvalue())
        self.assertIn("date", err.getvalue())
        self.assertIn("era", err.getvalue())

    def test_publish_noop_reconciles_index(self):
        # Identical-content re-publish after the index entry was
        # removed (drift/corruption) must restore the entry, exit 0.
        rc, _, _ = self._publish("20260905-v6", "2026-09-05")
        self.assertEqual(rc, 0)
        index = json.loads(
            (self.records / "index.json").read_text(encoding="utf-8")
        )
        index["runs"] = [
            r for r in index["runs"] if r["run_id"] != "20260905-v6"
        ]
        (self.records / "index.json").write_text(
            json.dumps(index) + "\n", encoding="utf-8"
        )
        rc2, out, _ = self._publish("20260905-v6", "2026-09-05")
        self.assertEqual(rc2, 0)
        self.assertIn("reconciled", out)
        index2 = json.loads(
            (self.records / "index.json").read_text(encoding="utf-8")
        )
        self.assertEqual(
            [r["run_id"] for r in index2["runs"]], ["20260905-v6"]
        )

    # -- Completeness gate (task 2.7): validation runs against the
    #    EXPECTED roster persisted in run.json, not the observed cells.

    def test_publish_clean_on_complete(self):
        # Full t2-json roster (warm-applicable), m1+m2 for every cell.
        cells = _metrics_cells("t2-json", FULL_CONTENDERS)
        rc, out, err = self._publish("20260905-v7", "2026-09-05", cells=cells)
        self.assertEqual(rc, 0)
        self.assertEqual(err, "")
        index = json.loads(
            (self.records / "index.json").read_text(encoding="utf-8")
        )
        self.assertEqual([r["run_id"] for r in index["runs"]], ["20260905-v7"])

    def test_publish_rejects_missing_metric(self):
        # One warm-applicable cell has m1 but no m2 (and no attempted
        # m2 with data): every other cell is complete, so the one gap
        # line must name exactly that cell and the m2 metric.
        roster = [f"t2-json/{c}" for c in FULL_CONTENDERS]
        complete = [c for c in FULL_CONTENDERS if c != "rust-camel-lib"]
        cells = (
            _metrics_cells("t2-json", complete)
            + _metrics_cells("t2-json", ["rust-camel-lib"], metrics=("m1",))
        )
        rc, _, err = self._publish(
            "20260905-v8", "2026-09-05", cells=cells,
            expected_cells=roster,
        )
        self.assertEqual(rc, 1)
        self.assertIn("t2-json/rust-camel-lib/m2", err)
        # The complete cells are not blamed.
        self.assertNotIn("t2-json/node-fastify/m2", err)
        self.assertNotIn("t2-json/rust-camel-lib/m1", err)

    def test_publish_rejects_wholly_absent_cell(self):
        # One expected cell produced NOTHING (no cell entry at all).
        # The gap is named from the roster identity — validation is
        # against the EXPECTED roster, not the observed cells.
        roster = [f"t2-json/{c}" for c in FULL_CONTENDERS]
        cells = _metrics_cells(
            "t2-json",
            [c for c in FULL_CONTENDERS if c != "camel-standalone-dsl"],
        )
        rc, _, err = self._publish(
            "20260905-v9", "2026-09-05", cells=cells,
            expected_cells=roster,
        )
        self.assertEqual(rc, 1)
        self.assertIn("t2-json/camel-standalone-dsl/m1", err)
        self.assertIn("t2-json/camel-standalone-dsl/m2", err)

    def test_publish_rejects_wholly_absent_scenario(self):
        # Every cell of one scenario is absent: the roster identities
        # (not a count) let the publisher reconstruct and name each
        # missing cell of the vanished scenario.
        roster = (
            [f"t2-json/{c}" for c in FULL_CONTENDERS]
            + [f"split-aggregate/{c}" for c in FULL_CONTENDERS]
        )
        cells = _metrics_cells("t2-json", FULL_CONTENDERS)
        rc, _, err = self._publish(
            "20260905-v10", "2026-09-05", cells=cells,
            expected_cells=roster,
        )
        self.assertEqual(rc, 1)
        for contender in FULL_CONTENDERS:
            self.assertIn(f"split-aggregate/{contender}/m1", err)
            self.assertIn(f"split-aggregate/{contender}/m2", err)
        # The present scenario is not blamed.
        self.assertNotIn("t2-json/", err)

    def test_startup_warm_na_not_gap(self):
        # startup-minimal is cold-only by design: m1-only cells are
        # complete, no m2 complaint.
        cells = _metrics_cells("startup-minimal", FULL_CONTENDERS,
                               metrics=("m1",))
        rc, out, err = self._publish("20260905-v11", "2026-09-05",
                                     cells=cells)
        self.assertEqual(rc, 0)
        self.assertEqual(err, "")

    def test_publish_m2_attempted_counts_as_present(self):
        # bd rc-tpig: a cell with n>0 records but
        # status=failed insufficient-samples is PRESENT m2 data — the
        # gate must not fail closed on the sample-count status.
        roster = [
            "split-aggregate/rust-camel-lib",
            "split-aggregate/rust-camel-cli",
        ]
        cells = (
            _metrics_cells("split-aggregate", ["rust-camel-lib"])
            + _metrics_cells("split-aggregate", ["rust-camel-cli"],
                             metrics=("m1",))
        )
        rc, _, err = self._publish(
            "20260905-v12", "2026-09-05", cells=cells,
            expected_cells=roster,
            m2_attempted_cells=["split-aggregate/rust-camel-cli"],
        )
        self.assertEqual(rc, 0)
        self.assertEqual(err, "")

    # -- Attempted m2 cells (schema_version 2): the publisher
    #    re-validates the derived shape — status/reason/rounds, no
    #    latency fields — and invalid shapes fail closed as MISSING.

    def test_publish_accepts_attempted_cells(self):
        # One warm-applicable cell attempted (unconverged, nonempty
        # reason, no latency fields, rounds=2): publish accepts and
        # the success output splits measured vs attempted.
        rc, out, err = self._publish(
            "20260905T140000Z", "2026-09-05",
            cells=_split_cells_with_attempt(_attempted_cell()),
            expected_cells=_split_roster(),
        )
        self.assertEqual(rc, 0)
        self.assertEqual(err, "")
        self.assertIn("attempted", out)
        self.assertIn("present m2 8/8: 7 measured, 1 attempted", out)
        published = self.records / "20260905T140000Z"
        self.assertTrue((published / "run.json").is_file())
        self.assertTrue((published / "summary.md").is_file())
        index = json.loads(
            (self.records / "index.json").read_text(encoding="utf-8")
        )
        self.assertEqual(
            [r["run_id"] for r in index["runs"]], ["20260905T140000Z"]
        )

    def test_publish_rejects_unknown_status(self):
        # An unknown status is not a publishable shape: fail closed
        # naming the cell.
        rc, _, err = self._publish(
            "20260905T140001Z", "2026-09-05",
            cells=_split_cells_with_attempt(_attempted_cell(status="weird")),
            expected_cells=_split_roster(),
        )
        self.assertNotEqual(rc, 0)
        self.assertIn("split-aggregate/rust-camel-lib/m2", err)

    def test_publish_rejects_status_with_latency(self):
        # status mixed with ANY latency field is invalid.
        for i, (field, value) in enumerate((
            ("unit", "ns"),
            ("median", 11.0),
            ("round_values", [10.0, 12.0]),
        )):
            with self.subTest(field=field):
                cell = _attempted_cell()
                cell[field] = value
                rc, _, err = self._publish(
                    f"20260905T1401{i:02d}Z", "2026-09-05",
                    cells=_split_cells_with_attempt(cell),
                    expected_cells=_split_roster(),
                )
                self.assertNotEqual(rc, 0)
                self.assertIn("split-aggregate/rust-camel-lib/m2", err)

    def test_publish_rejects_empty_reason(self):
        rc, _, err = self._publish(
            "20260905T140003Z", "2026-09-05",
            cells=_split_cells_with_attempt(_attempted_cell(reason="")),
            expected_cells=_split_roster(),
        )
        self.assertNotEqual(rc, 0)
        self.assertIn("split-aggregate/rust-camel-lib/m2", err)

    def test_publish_rejects_bare_metric_cell(self):
        # A cell carrying only identity + metric validates nothing.
        cell = {
            "scenario": "split-aggregate",
            "contender": "rust-camel-lib",
            "metric": "m2",
        }
        rc, _, err = self._publish(
            "20260905T140004Z", "2026-09-05",
            cells=_split_cells_with_attempt(cell),
            expected_cells=_split_roster(),
        )
        self.assertNotEqual(rc, 0)
        self.assertIn("split-aggregate/rust-camel-lib/m2", err)

    def test_publish_rejects_attempted_without_valid_rounds(self):
        # rounds must be a positive integer — missing, 0, True (a
        # bool) and "2" (a str) are all invalid.
        variants = {"missing": None, "zero": 0, "bool": True, "str": "2"}
        for i, (name, rounds) in enumerate(variants.items()):
            with self.subTest(rounds=name):
                cell = _attempted_cell()
                if rounds is None:
                    del cell["rounds"]
                else:
                    cell["rounds"] = rounds
                rc, _, err = self._publish(
                    f"20260905T1402{i:02d}Z", "2026-09-05",
                    cells=_split_cells_with_attempt(cell),
                    expected_cells=_split_roster(),
                )
                self.assertNotEqual(rc, 0)
                self.assertIn("split-aggregate/rust-camel-lib/m2", err)

    # -- Back-compat: the extension is additive and one-way; v1
    #    records stay readable by v2 tooling.

    def test_v1_record_still_validates(self):
        # A complete schema_version 1 record (all measured, no status
        # fields) publishes and checks accepted, unchanged.
        cells = _metrics_cells("t2-json", FULL_CONTENDERS)
        rc, out, err = self._publish(
            "20260901-v5", "2026-09-01", cells=cells, schema_version=1
        )
        self.assertEqual(rc, 0)
        self.assertEqual(err, "")
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = summarize.main(["--check", str(self.records)])
        self.assertEqual(rc, 0)

    def test_mixed_v1_v2_index_rebuild(self):
        # One v1 + one v2 record (with an attempted cell) in the same
        # records dir: the rebuild succeeds, index_schema_version stays
        # 1, both entries present.
        rc1, _, err1 = self._publish(
            "20260901-v5", "2026-09-01",
            cells=_metrics_cells("t2-json", FULL_CONTENDERS),
            schema_version=1,
        )
        self.assertEqual(rc1, 0)
        self.assertEqual(err1, "")
        rc2, _, err2 = self._publish(
            "20260905T140000Z", "2026-09-05",
            cells=_split_cells_with_attempt(_attempted_cell()),
            expected_cells=_split_roster(),
            schema_version=2,
        )
        self.assertEqual(rc2, 0)
        self.assertEqual(err2, "")
        index = json.loads(
            (self.records / "index.json").read_text(encoding="utf-8")
        )
        self.assertEqual(index["index_schema_version"], 1)
        self.assertEqual(
            [r["run_id"] for r in index["runs"]],
            ["20260901-v5", "20260905T140000Z"],
        )

    def test_index_dir_crosscheck(self):
        # A run dir without an index entry is an orphan: --check must
        # exit 1 naming it.
        make_record_dir(self.records, "20260905-v6", "2026-09-05")
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = summarize.main(["--check", str(self.records)])
        self.assertEqual(rc, 1)
        self.assertIn("20260905-v6", err.getvalue())

    def test_bench_facade_wired(self):
        # `bench summarize` with no args must be a usage error (exit 2)
        # from the wired summarize.py, never the old "not implemented"
        # stub.
        bench = Path(__file__).resolve().parent.parent / "bench"
        proc = subprocess.run(
            ["bash", str(bench), "summarize"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(proc.returncode, 2)
        self.assertNotIn("not implemented", proc.stderr)
        self.assertIn("usage", proc.stderr.lower())

    # -- End-to-end replica of the canonical-run gap families (Task
    #    3.1): the REAL summarize -> publish chain over a synthetic run
    #    dir — no hand-built run.json. Chain-guards: both FAIL if the
    #    classifier wiring (summarize) or the attempted-shape gate
    #    (publish) is broken.

    def _summarize_e2e(self, run, run_id):
        """Real summarize path (main(), summarize mode) over the
        synthetic run dir; returns the summarized record dir."""
        meta_path = self.root / "meta.json"
        meta_path.write_text(
            json.dumps(dict(E2E_META, run_id=run_id)), encoding="utf-8"
        )
        out = self.root / f"summarized-{run_id}"
        err = io.StringIO()
        with mock.patch.dict(
            os.environ,
            {"BENCH_PAYLOAD_DIGEST_BIN": str(
                _unknown_scenario_digest_stub(self.root))},
        ):
            with contextlib.redirect_stderr(err):
                with contextlib.redirect_stdout(io.StringIO()):
                    rc = summarize.main([
                        "--run-dir", str(run),
                        "--meta", str(meta_path),
                        "--out-dir", str(out),
                    ])
        self.assertEqual(rc, 0, err.getvalue())
        return out

    def test_e2e_canonical_gap_families_publish(self):
        # startup-minimal (cold-only) + xslt-bridge (6 contenders):
        # m1 everywhere; xslt-bridge m2 = 4 measured + 1 unconverged +
        # 1 attempted-timeout. Summarize -> publish: exit 0, split
        # line exact, both attempted statuses in run.json, one index
        # entry.
        run = make_e2e_gap_families_run(self.root)
        source = self._summarize_e2e(run, "20260906T060000Z")
        err = io.StringIO()
        out = io.StringIO()
        with contextlib.redirect_stderr(err):
            with contextlib.redirect_stdout(out):
                rc = summarize.main([
                    "--publish",
                    "--run-dir", str(source),
                    "--records-dir", str(self.records),
                ])
        self.assertEqual(rc, 0, err.getvalue())
        self.assertIn(
            "present m2 6/6: 4 measured, 2 attempted", out.getvalue()
        )
        published = self.records / "20260906T060000Z"
        record = json.loads(
            (published / "run.json").read_text(encoding="utf-8")
        )
        by_identity = {
            (c["scenario"], c["contender"]): c
            for c in record["cells"] if c["metric"] == "m2"
        }
        unconv = by_identity[("xslt-bridge", "rust-camel-lib")]
        self.assertEqual(unconv["status"], "unconverged")
        self.assertEqual(
            unconv["reason"], "status=failed reason=measure-a-error"
        )
        self.assertEqual(unconv["rounds"], 2)
        timeout = by_identity[("xslt-bridge", "rust-camel-cli")]
        self.assertEqual(timeout["status"], "attempted-timeout")
        self.assertEqual(
            timeout["reason"],
            "# probe reason: no BENCH_LATENCY within 30s timeout",
        )
        self.assertEqual(timeout["rounds"], 2)
        for latency in ("round_values", "median", "unit"):
            self.assertNotIn(latency, unconv)
            self.assertNotIn(latency, timeout)
        index = json.loads(
            (self.records / "index.json").read_text(encoding="utf-8")
        )
        self.assertEqual(
            [r["run_id"] for r in index["runs"]], ["20260906T060000Z"]
        )

    def test_e2e_evidenceless_gap_blocks(self):
        # Same fixture minus the exit-codes.txt evidence: the timeout
        # cell is evidence-less (no summary, no status) -> publish
        # exits nonzero naming exactly that cell.
        run = make_e2e_gap_families_run(
            self.root, run_id="20260906T070000Z",
            with_timeout_evidence=False,
        )
        source = self._summarize_e2e(run, "20260906T070000Z")
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = summarize.main([
                "--publish",
                "--run-dir", str(source),
                "--records-dir", str(self.records),
            ])
        self.assertNotEqual(rc, 0)
        gaps = [
            line for line in err.getvalue().splitlines()
            if line.startswith("error:   missing ")
        ]
        self.assertEqual(
            gaps, ["error:   missing xslt-bridge/rust-camel-cli/m2"]
        )


if __name__ == "__main__":
    unittest.main()
