"""Tests for publish/check modes — stdlib unittest, synthetic fixtures.

Publish/check operate on SUMMARIZED record dirs (run.json +
summary.md), so fixtures here are hand-built records emitted through
`emit_json`/`emit_summary` — no payload-digest subprocesses, unlike
test_summarize.py which exercises the raw run-dir pipeline.
"""
import contextlib
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

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


def make_record_dir(root, run_id, date, cells=None, expected_cells=None,
                    m2_attempted_cells=None):
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
        "schema_version": 1,
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
                 m2_attempted_cells=None):
        source = make_record_dir(
            self.root, run_id, date, cells=cells,
            expected_cells=expected_cells,
            m2_attempted_cells=m2_attempted_cells,
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


if __name__ == "__main__":
    unittest.main()
