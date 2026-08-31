"""Tests for summarize.py — stdlib unittest, synthetic fixtures only."""
import contextlib
import io
import json
import os
import shutil
import stat
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import summarize

META = {
    "era": "2",
    "git_commit": "a" * 40,
    "container_digest": None,
    "run_id": "20260905T150000Z",
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

# Goldens mirrored from payload.rs (change bench-missing-cells): the
# t2-json era-default 32 KiB canonical body and the split-aggregate
# canonical array. The validating stub prints these for `shared`, so
# assertions in this file pin the REAL binary contract, not a fantasy
# digest.
T2_JSON_SHARED_GOLDEN = (
    "a0db69e1146a29b0b25ca22435e51f39e271ecb1ac4ec1cee0ead3212eae10e9"
)
SPLIT_AGGREGATE_GOLDEN = (
    "123444b475c48473309ed966eb69896c6725429021a5a5d2e0eaa0a77a159316"
)
# Distinct marker for numeric (axis) classes so tests can tell the
# shared branch from the numeric branch of the stub.
STUB_NUMERIC_DIGEST_HEX = "cd" * 32


# Real era-1 run dir (git-tracked regression fixture): flat
# `<scenario>_<contender>/` cell dirs holding m3+m4 summaries with
# `cell` fields in slash form, NO samples.txt, plus measurement_order
# and provenance at the run root.
ERA1_FIXTURE = (
    Path(__file__).resolve().parent.parent
    / "attic" / "results-era-1" / "20260723T161422Z"
)


def make_run(root):
    r"""Synthetic run dir matching the REAL run.sh output layout.

    Every artifact lives in a FLAT `<scenario>_<contender>/` cell dir
    at the run root (run.sh does `cell_safe="${cell//\//_}"`). Two
    scenarios: `t2-json` (has a canonical payload contract — the
    stub returns a digest) and `startup-minimal` (no canonical payload
    contract — the stub fails with `unknown scenario`, so the cell
    records `input_sha256: null`).
    """
    run = root / "20260905T120000Z"
    for scenario, contenders in (
        ("t2-json", (
            ("rust-camel-lib", [100.0, 102.0]),
            ("camel-standalone-dsl", [50.0, 51.0]),
        )),
        ("startup-minimal", (("rust-camel-lib", [10.0, 11.0]),)),
    ):
        for contender, means in contenders:
            cell = run / f"{scenario}_{contender}"
            cell.mkdir(parents=True)
            (cell / "m3-summary.json").write_text(
                json.dumps({
                    "cell": f"{scenario}/{contender}",
                    "status": "ok",
                    "median_mean_msgs_per_sec": sum(means) / len(means),
                    "min_mean": min(means),
                    "max_mean": max(means),
                    "per_round_means": means,
                    "rounds": 2,
                    "duration_secs": 10.0,
                    "warmup_secs": 2.0,
                }),
                encoding="utf-8",
            )
    m1 = run / "t2-json_rust-camel-lib"
    (m1 / "samples.txt").write_text(
        "startup-ms rss-kb\n12 900\n14 950\n", encoding="utf-8"
    )
    return run


class SummarizeTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.run_dir = make_run(self.root)
        self.stub_digest = self._write_stub_digest()

    def _write_stub_digest(self):
        """Class-validating payload-digest stub mirroring the real
        bench-loadgen binary's contract (accepts both `--k v` and
        `--k=v` flag forms, like cli.rs `parse_flags`):

        - t2-json: `shared` -> era-default 32 KiB golden; numeric
          classes in the axis set (1024, 32768, 262144, 1048576)
          accepted; anything else rejected with rc=2 and
          `unknown payload class` on stderr.
        - split-aggregate: `shared` -> canonical array golden; any
          other class rejected rc=2.
        - any other scenario: rc=2 with `unknown scenario` on stderr
          (no canonical payload contract -> summarize.py records
          `input_sha256: null`).

        Every invocation appends a line to an `invocations` file next
        to the stub so tests can verify memoization (one subprocess
        per (scenario, payload-class) pair, not per cell).
        """
        stub = self.root / "stub-payload-digest.sh"
        stub.write_text(
            "#!/bin/sh\n"
            'scenario=""\n'
            'class=""\n'
            'prev=""\n'
            'for a in "$@"; do\n'
            '  if [ "$prev" = "--scenario" ]; then scenario="$a"; fi\n'
            '  if [ "$prev" = "--payload-class" ]; then class="$a"; fi\n'
            '  case "$a" in\n'
            '    --scenario=*) scenario="${a#--scenario=}";;\n'
            '    --payload-class=*) class="${a#--payload-class=}";;\n'
            '  esac\n'
            '  prev="$a"\n'
            "done\n"
            'echo x >> "$(dirname "$0")/invocations"\n'
            'case "$scenario" in\n'
            "  t2-json)\n"
            '    case "$class" in\n'
            "      shared)\n"
            "        echo " + T2_JSON_SHARED_GOLDEN + "\n"
            "        ;;\n"
            "      1024|32768|262144|1048576)\n"
            "        echo " + STUB_NUMERIC_DIGEST_HEX + "\n"
            "        ;;\n"
            "      *)\n"
            '        echo "payload-digest: unknown payload class '
            '\\"$class\\": t2-json classes are \\"shared\\" or byte '
            'sizes 1024, 32768, 262144, 1048576" >&2\n'
            "        exit 2\n"
            "        ;;\n"
            "    esac\n"
            "    ;;\n"
            "  split-aggregate)\n"
            '    case "$class" in\n'
            "      shared)\n"
            "        echo " + SPLIT_AGGREGATE_GOLDEN + "\n"
            "        ;;\n"
            "      *)\n"
            '        echo "payload-digest: unknown payload class '
            '\\"$class\\": split-aggregate classes are \\"shared\\"" >&2\n'
            "        exit 2\n"
            "        ;;\n"
            "    esac\n"
            "    ;;\n"
            "  *)\n"
            '    echo "payload-digest: unknown scenario \\"$scenario\\": '
            'payload-digest supports t2-json and split-aggregate" >&2\n'
            "    exit 2\n"
            "    ;;\n"
            "esac\n",
            encoding="utf-8",
        )
        stub.chmod(
            stub.stat().st_mode
            | stat.S_IXUSR
            | stat.S_IXGRP
            | stat.S_IXOTH
        )
        return stub

    def _record(self):
        env = {"BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest)}
        with mock.patch.dict(os.environ, env):
            return summarize.build_record(self.run_dir, dict(META))

    def _make_stub(self, name, text):
        """Executable shell stub under the test tmp root."""
        stub = self.root / name
        stub.write_text(text, encoding="utf-8")
        stub.chmod(
            stub.stat().st_mode
            | stat.S_IXUSR
            | stat.S_IXGRP
            | stat.S_IXOTH
        )
        return stub

    def test_build_record_median(self):
        record = self._record()
        cell = next(
            c for c in record["cells"]
            if c["metric"] == "m3" and c["contender"] == "rust-camel-lib"
            and c["scenario"] == "t2-json"
        )
        self.assertEqual(cell["median"], 101.0)

    def test_determinism(self):
        out_a = self.root / "out-a"
        out_b = self.root / "out-b"
        summarize.emit_json(self._record(), out_a)
        summarize.emit_json(self._record(), out_b)
        self.assertEqual(
            (out_a / "run.json").read_bytes(),
            (out_b / "run.json").read_bytes(),
        )

    def test_schema_fields(self):
        record = self._record()
        for key in (
            "schema_version",
            "run_id",
            "era",
            "date",
            "git_commit",
            "container_digest",
            "host_provenance",
            "protocol",
            "cells",
            "ratios",
        ):
            self.assertIn(key, record)
        self.assertEqual(record["schema_version"], 1)
        self.assertIsInstance(record["era"], str)
        # run_id comes straight from meta (launch timestamp; no
        # sequence composition for new runs).
        self.assertEqual(record["run_id"], "20260905T150000Z")
        # input_sha256 is the canonical INPUT digest (payload-digest),
        # not a hash of the summary artifact. Scenarios with a
        # canonical payload contract (t2-json) get the era-default
        # golden for `shared` (the 32 KiB body); the startup-minimal
        # m1 flat dir is t2-json-prefixed, so it gets the digest too.
        # startup-minimal cells record null.
        for cell in record["cells"]:
            if cell["scenario"] == "startup-minimal":
                self.assertIsNone(
                    cell["input_sha256"],
                    "scenario without canonical payload contract must be null",
                )
            else:
                self.assertEqual(
                    cell["input_sha256"],
                    f"sha256:{T2_JSON_SHARED_GOLDEN}",
                )

    def test_input_sha256_null_warns_for_unknown_contract(self):
        err = io.StringIO()
        env = {"BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest)}
        with mock.patch.dict(os.environ, env):
            with contextlib.redirect_stderr(err):
                record = summarize.build_record(self.run_dir, dict(META))
        self.assertIn(
            "input_sha256: no canonical payload contract "
            "for scenario startup-minimal",
            err.getvalue(),
        )
        cell = next(
            c for c in record["cells"] if c["scenario"] == "startup-minimal"
        )
        self.assertIsNone(cell["input_sha256"])

    def test_t2_json_seam_input_sha256_is_era_default_golden(self):
        # The seam end-to-end at the contract level: a t2-json-shaped
        # run dir through build_record with the class-validating stub
        # records the REAL binary's era-default golden for
        # `shared` — the exact failure mode this guards against is
        # summarize.py passing a class the binary rejects, which
        # would crash build_record for every t2-json cell.
        record = self._record()
        t2_cells = [
            c for c in record["cells"] if c["scenario"] == "t2-json"
        ]
        self.assertTrue(t2_cells, "fixture must produce t2-json cells")
        for cell in t2_cells:
            self.assertEqual(
                cell["input_sha256"],
                f"sha256:{T2_JSON_SHARED_GOLDEN}",
                f"cell {cell['scenario']}/{cell['contender']}/"
                f"{cell['metric']} must carry the era-default golden",
            )

    def test_stub_rejects_invalid_payload_class(self):
        # The stub mimics the binary: an invalid class is rc=2 with
        # `unknown payload class` on stderr -> LOUD RuntimeError, not
        # a silent null.
        env = {"BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest)}
        with mock.patch.dict(os.environ, env):
            with self.assertRaises(RuntimeError) as cm:
                summarize.input_sha256("t2-json", "garbage")
        self.assertIn("t2-json/garbage", str(cm.exception))
        self.assertIn("rc=2", str(cm.exception))
        self.assertIn("unknown payload class", str(cm.exception))
        with mock.patch.dict(os.environ, env):
            with self.assertRaises(RuntimeError) as cm:
                summarize.input_sha256("split-aggregate", "32768")
        self.assertIn("split-aggregate/32768", str(cm.exception))
        self.assertIn("unknown payload class", str(cm.exception))

    def test_stub_accepts_numeric_axis_class(self):
        # Numeric classes remain valid for t2-json (payload-size axis
        # runs); the stub answers from its numeric branch.
        env = {"BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest)}
        with mock.patch.dict(os.environ, env):
            digest = summarize.input_sha256("t2-json", "32768")
        self.assertEqual(digest, f"sha256:{STUB_NUMERIC_DIGEST_HEX}")

    def test_payload_digest_memoized_per_pair(self):
        # One payload-digest subprocess per (scenario, payload-class)
        # pair: this fixture has 3 t2-json cells (2 m3 + 1 m1) and 1
        # startup-minimal cell -> exactly 2 invocations, not 4.
        self._record()
        invocations = self.root / "invocations"
        lines = invocations.read_text(encoding="utf-8").splitlines()
        self.assertEqual(
            len(lines),
            2,
            f"expected one digest subprocess per (scenario, class) "
            f"pair, got {len(lines)}",
        )

    def test_run_id_explicit_wins(self):
        meta = dict(META, run_id="20260905-v9")
        env = {"BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest)}
        with mock.patch.dict(os.environ, env):
            record = summarize.build_record(self.run_dir, meta)
        self.assertEqual(record["run_id"], "20260905-v9")

    def test_run_id_required_legacy_run_seq_composes(self):
        meta = dict(META)
        del meta["run_id"]
        with mock.patch.dict(os.environ, {"BENCH_PAYLOAD_DIGEST_BIN": "true"}):
            with self.assertRaises(ValueError):
                summarize.build_record(self.run_dir, meta)
        # Legacy metas (pre-2026-08-31) carry run_seq instead; the
        # <YYYYMMDD>-v<N> composition still works for them.
        legacy = dict(meta, run_seq=5)
        env = {"BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest)}
        with mock.patch.dict(os.environ, env):
            record = summarize.build_record(self.run_dir, legacy)
        self.assertEqual(record["run_id"], "20260905-v5")

    def test_round_values_coerces_floats_and_warns(self):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            vals = summarize.round_values([1, "2.5", "oops", 3], "s/c/m")
        self.assertEqual(repr(vals), "[1.0, 2.5, 3.0]")
        self.assertTrue(all(isinstance(v, float) for v in vals))
        self.assertIn("s/c/m: skipping non-numeric round value 'oops'", err.getvalue())

    def test_input_digest_loud_on_failure(self):
        # `false` exits nonzero with no `unknown scenario` in stderr
        # -> RuntimeError naming scenario/class.
        with mock.patch.dict(os.environ, {"BENCH_PAYLOAD_DIGEST_BIN": "false"}):
            with self.assertRaises(RuntimeError) as cm:
                summarize.input_sha256("t2-json", "shared")
        self.assertIn("t2-json/shared", str(cm.exception))

    def test_input_digest_loud_on_missing_binary(self):
        # A missing binary is NEVER forgiven as `null` — hard fail.
        with mock.patch.dict(
            os.environ,
            {"BENCH_PAYLOAD_DIGEST_BIN": str(self.root / "no-such-bin")},
        ):
            with self.assertRaises(RuntimeError) as cm:
                summarize.input_sha256("t2-json", "shared")
        self.assertIn("payload-digest unavailable", str(cm.exception))

    def test_input_digest_loud_on_other_nonzero_error(self):
        # Non-zero exit whose stderr does NOT say `unknown scenario`
        # (e.g. a broken build) is a hard fail, not a null.
        stub = self.root / "stub-digest-broken.sh"
        stub.write_text(
            "#!/bin/sh\necho 'error: could not parse Cargo.toml' >&2\nexit 3\n",
            encoding="utf-8",
        )
        stub.chmod(
            stub.stat().st_mode
            | stat.S_IXUSR
            | stat.S_IXGRP
            | stat.S_IXOTH
        )
        with mock.patch.dict(os.environ, {"BENCH_PAYLOAD_DIGEST_BIN": str(stub)}):
            with self.assertRaises(RuntimeError) as cm:
                summarize.input_sha256("t2-json", "shared")
        self.assertIn("rc=3", str(cm.exception))

    def test_m1_flat_dir_longest_prefix_split(self):
        run = self.root / "prefix-run"
        cell = run / "t2-json-ext_rust-camel-lib"
        cell.mkdir(parents=True)
        (cell / "m3-summary.json").write_text(
            json.dumps({
                "cell": "t2-json-ext/rust-camel-lib",
                "status": "ok",
                "per_round_means": [10.0],
                "rounds": 1,
            }),
            encoding="utf-8",
        )
        flat = run / "t2-json-ext_some_contender"
        flat.mkdir()
        (flat / "samples.txt").write_text("11 900\n", encoding="utf-8")
        with mock.patch.dict(
            os.environ, {"BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest)}
        ):
            cells = summarize.load_cells(run)
        m1 = next(c for c in cells if c["metric"] == "m1")
        self.assertEqual(m1["scenario"], "t2-json-ext")
        self.assertEqual(m1["contender"], "some_contender")

    def test_m1_flat_dir_no_prefix_match_is_loud(self):
        run = self.root / "orphan-run"
        run.mkdir()
        flat = run / "unknown-scenario_x"
        flat.mkdir()
        (flat / "samples.txt").write_text("11 900\n", encoding="utf-8")
        with self.assertRaises(ValueError):
            summarize.load_cells(run)

    @unittest.skipUnless(
        ERA1_FIXTURE.is_dir(), "era-1 fixture run dir not present"
    )
    def test_era1_fixture_shape_loads_cells(self):
        # Regression for the fantasy-layout bug: the REAL era-1 run dir
        # (flat dirs, m3+m4 summaries with `cell` fields, NO
        # samples.txt) must load >0 cells with the correct
        # scenario/contender split taken from the `cell` fields — the
        # old nested-dir reader resolved this shape to 0 cells
        # silently.
        run = shutil.copytree(ERA1_FIXTURE, self.root / "20260723T161422Z")
        env = {"BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest)}
        with mock.patch.dict(os.environ, env):
            record = summarize.build_record(
                run, dict(META, run_id="20260723-v4")
            )
        cells = record["cells"]
        self.assertGreater(len(cells), 0)
        # 6 contenders x (m3 + m4) = 12 cells, all http-server.
        self.assertEqual(len(cells), 12)
        self.assertTrue(all(c["scenario"] == "http-server" for c in cells))
        self.assertEqual(
            {c["contender"] for c in cells},
            {
                "camel-quarkus-dsl-native",
                "camel-quarkus-yaml-native",
                "camel-standalone-dsl",
                "camel-standalone-yaml",
                "rust-camel-cli",
                "rust-camel-lib",
            },
        )
        keys = {(c["scenario"], c["contender"], c["metric"]) for c in cells}
        self.assertIn(("http-server", "camel-standalone-dsl", "m3"), keys)
        self.assertIn(("http-server", "rust-camel-lib", "m4"), keys)
        # http-server has no canonical payload contract -> null digest.
        self.assertTrue(all(c["input_sha256"] is None for c in cells))

    def test_cell_identity_comes_from_cell_field_not_dir_name(self):
        run = self.root / "renamed-run"
        cell = run / "totally-renamed-dir"
        cell.mkdir(parents=True)
        (cell / "m3-summary.json").write_text(
            json.dumps({
                "cell": "t2-json/rust-camel-lib",
                "status": "ok",
                "per_round_means": [10.0, 20.0],
                "rounds": 2,
            }),
            encoding="utf-8",
        )
        env = {"BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest)}
        with mock.patch.dict(os.environ, env):
            cells = summarize.load_cells(run)
        self.assertEqual(len(cells), 1)
        self.assertEqual(cells[0]["scenario"], "t2-json")
        self.assertEqual(cells[0]["contender"], "rust-camel-lib")
        self.assertEqual(cells[0]["median"], 15.0)

    def test_pure_m1_run_resolves_from_meta_scenarios(self):
        run = self.root / "20260905T160000Z"
        for name in ("t2-json_rust-camel-lib", "startup-minimal_rust-camel-lib"):
            cell = run / name
            cell.mkdir(parents=True)
            (cell / "samples.txt").write_text(
                "12 900\n14 950\n", encoding="utf-8"
            )
        env = {"BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest)}
        for meta in (
            dict(META, scenarios="t2-json,startup-minimal"),
            dict(META, subset="t2-json,startup-minimal"),  # legacy key
        ):
            with mock.patch.dict(os.environ, env):
                cells = summarize.load_cells(run, meta)
            self.assertEqual(
                {(c["scenario"], c["contender"], c["metric"]) for c in cells},
                {
                    ("t2-json", "rust-camel-lib", "m1"),
                    ("startup-minimal", "rust-camel-lib", "m1"),
                },
            )

    def test_m1_samples_tolerates_null_rss(self):
        # Wrapper-launched cells (see run.sh CELL_RSS_WRAPPER) write
        # `<ms> null`: time -v measured the wrapper, not the
        # contender. _m1_samples reads only the ms column.
        f = self.root / "null-rss-samples.txt"
        f.write_text("34 null\n36 null\n32 null\n", encoding="utf-8")
        self.assertEqual(summarize._m1_samples(f), [34.0, 36.0, 32.0])

    def test_zero_cells_is_loud(self):
        run = self.root / "20260905T170000Z"
        (run / "http-server_some-contender").mkdir(parents=True)  # empty
        with self.assertRaises(ValueError) as cm:
            summarize.load_cells(run)
        self.assertIn("0 measurement cells", str(cm.exception))
        self.assertIn("20260905T170000Z", str(cm.exception))

    def test_zero_cells_cli_exits_nonzero(self):
        run = self.root / "20260905T180000Z"
        (run / "http-server_x").mkdir(parents=True)
        meta_path = self.root / "meta.json"
        meta_path.write_text(json.dumps(META), encoding="utf-8")
        out = self.root / "out"
        with mock.patch.dict(
            os.environ, {"BENCH_PAYLOAD_DIGEST_BIN": "true"}
        ):
            rc = summarize.main([
                "--run-dir", str(run),
                "--meta", str(meta_path),
                "--out-dir", str(out),
            ])
        self.assertEqual(rc, 1)
        self.assertFalse((out / "run.json").exists())

    def _ratio_stub(self, name, num_arg="na", den_arg="nd",
                    point=1.5, ci_lo=1.4, ci_hi=1.6):
        """aggregate-ratios stub speaking the REAL binary vocabulary:
        numerator/denominator are CELL-DIR basenames of the two input
        m3-summary.json paths (ratios.rs RatioReport `cell_dir`)."""
        return self._make_stub(
            name,
            "#!/bin/sh\n"
            'na=$(basename "$(dirname "$1")")\n'
            'nd=$(basename "$(dirname "$2")")\n'
            'printf \'{"numerator":"%s","denominator":"%s","metric":"m3",'
            f'"point":{point},"ci_lo":{ci_lo},"ci_hi":{ci_hi},'
            '"method":"bootstrap-paired"}\\n\' "$' + num_arg + '" "$'
            + den_arg + '"\n',
        )

    def test_ratio_delegation(self):
        # The stub echoes the CELL-DIR basenames it received as
        # numerator/denominator — exactly what the real binary prints
        # for the now-flat paths (`t2-json_rust-camel-lib`, not the
        # bare contender). compute_ratios must normalize that
        # vocabulary to the SCHEMA's bare contender names while
        # keeping the PAIRING DIRECTION: rust-camel-lib is numerator.
        stub = self._ratio_stub("stub-ratios.sh")
        record = self._record()
        env = {
            "BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest),
            "BENCH_AGGREGATE_RATIOS_BIN": str(stub),
        }
        with mock.patch.dict(os.environ, env):
            record["ratios"] = summarize.compute_ratios(
                record, run_dir=self.run_dir
            )
        # Pinned pairing rule + name normalization: numerator is the
        # bare contender name.
        self.assertEqual(
            record["ratios"],
            [{
                "numerator": "rust-camel-lib",
                "denominator": "camel-standalone-dsl",
                "metric": "m3",
                "point": 1.5,
                "ci_lo": 1.4,
                "ci_hi": 1.6,
                "method": "bootstrap-paired",
            }],
        )

    def test_ratio_mirrored_when_binary_flips_direction(self):
        # If the binary reports the pair in the opposite direction
        # (its numerator basename == our denominator cell dir), the
        # values must mirror with the names: ratio(A,B) = 1/ratio(B,A)
        # and the CI bounds swap under inversion.
        stub = self._ratio_stub(
            "stub-ratios-flipped.sh", num_arg="nd", den_arg="na",
            point=2.0, ci_lo=1.5, ci_hi=3.0,
        )
        record = self._record()
        env = {
            "BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest),
            "BENCH_AGGREGATE_RATIOS_BIN": str(stub),
        }
        with mock.patch.dict(os.environ, env):
            record["ratios"] = summarize.compute_ratios(
                record, run_dir=self.run_dir
            )
        row = record["ratios"][0]
        self.assertEqual(row["numerator"], "rust-camel-lib")
        self.assertEqual(row["denominator"], "camel-standalone-dsl")
        self.assertEqual(row["point"], 0.5)
        self.assertAlmostEqual(row["ci_lo"], 1.0 / 3.0)
        self.assertAlmostEqual(row["ci_hi"], 1.0 / 1.5)
        self.assertEqual(row["method"], "bootstrap-paired")

    def test_known_ci_bounds_flow_to_run_json_and_summary(self):
        # Design exit "one ratio with known CI bounds flowing --json ->
        # run.json -> summary.md". Two crafted IDENTICAL per-round
        # arrays: the true paired ratio is exactly 1.0 and the
        # percentile bootstrap CI degenerates to [1.0, 1.0]; the stub
        # returns that true value while speaking the real binary's
        # cell-dir-basename vocabulary. The seam runs through main()
        # end-to-end (real argv parsing, real emit paths) without a
        # cargo dependency in the Python test env — the Rust side of
        # the contract is pinned by ratios.rs's own known-bounds
        # tests.
        run = self.root / "20260723T161422Z"  # era-1-shaped: no samples.txt
        for contender in ("rust-camel-lib", "camel-standalone-dsl"):
            cell = run / f"http-server_{contender}"
            cell.mkdir(parents=True)
            (cell / "m3-summary.json").write_text(
                json.dumps({
                    "cell": f"http-server/{contender}",
                    "status": "ok",
                    "per_round_means": [100.0, 100.0, 100.0],
                    "rounds": 3,
                }),
                encoding="utf-8",
            )
        stub = self._ratio_stub(
            "stub-ratios-one.sh", point=1.0, ci_lo=1.0, ci_hi=1.0
        )
        meta_path = self.root / "meta.json"
        meta_path.write_text(
            json.dumps(dict(META, run_id="20260723T161422Z")), encoding="utf-8"
        )
        out = self.root / "out"
        env = {
            "BENCH_PAYLOAD_DIGEST_BIN": str(self.stub_digest),
            "BENCH_AGGREGATE_RATIOS_BIN": str(stub),
        }
        with mock.patch.dict(os.environ, env):
            rc = summarize.main([
                "--run-dir", str(run),
                "--meta", str(meta_path),
                "--out-dir", str(out),
            ])
        self.assertEqual(rc, 0)
        record = json.loads((out / "run.json").read_text(encoding="utf-8"))
        self.assertEqual(record["run_id"], "20260723T161422Z")
        self.assertEqual(len(record["ratios"]), 1)
        row = record["ratios"][0]
        self.assertEqual(row["numerator"], "rust-camel-lib")
        self.assertEqual(row["denominator"], "camel-standalone-dsl")
        self.assertEqual(row["point"], 1.0)
        self.assertEqual(row["ci_lo"], 1.0)
        self.assertEqual(row["ci_hi"], 1.0)
        summary = (out / "summary.md").read_text(encoding="utf-8")
        self.assertIn(
            "| rust-camel-lib | camel-standalone-dsl | m3"
            " | 1.0 | 1.0 | 1.0 | bootstrap-paired |",
            summary,
        )


class DigestGuardCheckTest(unittest.TestCase):
    """`--check` rejects mutable/missing container digests (spec
    scenario "Digest recorded, tag rejected")."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.records = self.root / "records"
        self.records.mkdir()

    def _record_dir(self, container_digest, containerized=False):
        """One summarized record dir + its index entry, with the
        given digest/containerized flags; everything else valid."""
        record = {
            "schema_version": 1,
            "run_id": "20260905-v6",
            "era": "2",
            "date": "2026-09-05",
            "git_commit": "a" * 40,
            "container_digest": container_digest,
            "host_provenance": {
                "cpu_model": "test-cpu",
                "cores": 1,
                "kernel": "test-kernel",
                "containerized": containerized,
                "load": {"one": 0.0, "five": 0.0, "fifteen": 0.0},
            },
            "protocol": {
                "rounds": 1,
                "duration_secs": 1.0,
                "warmup_secs": 0.0,
                "order_seed": 0,
            },
            "cells": [{
                "scenario": "startup-minimal",
                "contender": "rust-camel-lib",
                "variant": "default",
                "payload_class": "shared",
                "metric": "m1",
                "round_values": [10.0, 12.0],
                "median": 11.0,
                "unit": "ms",
                "input_sha256": None,
            }],
            "ratios": [],
        }
        out = self.records / record["run_id"]
        summarize.emit_json(record, out)
        summarize.emit_summary(record, out)
        index = {
            "index_schema_version": 1,
            "runs": [summarize.index_entry(record)],
        }
        (self.records / "index.json").write_text(
            json.dumps(index) + "\n", encoding="utf-8"
        )
        return out

    def _check(self):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = summarize.main(["--check", str(self.records)])
        return rc, err.getvalue()

    def test_digest_rejects_latest(self):
        # A record referencing the mutable newest tag must fail the
        # records guard, naming the record.
        self._record_dir("runner:latest", containerized=True)
        rc, err = self._check()
        self.assertEqual(rc, 1)
        self.assertIn("20260905-v6", err)
        self.assertIn("mutable tag", err)

    def test_digest_rejects_any_mutable_tag_shape(self):
        # The guard rejects ANY non-digest string, not just
        # `:latest` — a versioned tag like `benchmark-runner:era2`
        # floats exactly like `latest` and must never appear in a
        # record (same shape check the DIGEST file gets).
        self._record_dir("benchmark-runner:era2", containerized=True)
        rc, err = self._check()
        self.assertEqual(rc, 1)
        self.assertIn("20260905-v6", err)
        self.assertIn("mutable tag", err)

    def test_digest_null_rejected_when_containerized(self):
        # A containerized run without any digest is equally rejected.
        self._record_dir(None, containerized=True)
        rc, err = self._check()
        self.assertEqual(rc, 1)
        self.assertIn("20260905-v6", err)
        self.assertIn("empty container_digest", err)

    def test_digest_sha256_accepted_when_containerized(self):
        # Control: a pinned sha256 digest on a containerized run is
        # green — the guard has no false positive.
        self._record_dir("sha256:" + "b" * 64, containerized=True)
        rc, err = self._check()
        self.assertEqual(rc, 0, err)

    def test_digest_null_accepted_when_not_containerized(self):
        # Host (non-containerized) runs legitimately record null.
        self._record_dir(None, containerized=False)
        rc, err = self._check()
        self.assertEqual(rc, 0, err)

    def test_digest_non_string_rejected(self):
        # Type hole closed: a digest that is neither a string nor
        # null is a malformed record, rejected even when it would be
        # truthy (the old str-only endswith check let it slip through).
        self._record_dir(12345, containerized=False)
        rc, err = self._check()
        self.assertEqual(rc, 1)
        self.assertIn("20260905-v6", err)
        self.assertIn("container_digest must be a string or null", err)


if __name__ == "__main__":
    unittest.main()
