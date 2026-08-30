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


def _cell(scenario="startup-minimal", contender="rust-camel-lib"):
    return {
        "scenario": scenario,
        "contender": contender,
        "variant": "default",
        "payload_class": "shared",
        "metric": "m1",
        "round_values": [10.0, 12.0],
        "median": 11.0,
        "unit": "ms",
        "input_sha256": None,
    }


def make_record_dir(root, run_id, date, cells=None):
    """Synthetic summarized record dir: run.json + generated summary.md."""
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
        "cells": cells if cells is not None else [_cell()],
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

    def _publish(self, run_id, date, cells=None):
        source = make_record_dir(self.root, run_id, date, cells=cells)
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
