"""Tests for the full-metric-set default (bd rc-awyoj).

Owner ruling 2026-09-16: a bare bench invocation must never silently
produce a partial record. The harness default metric set is therefore
the FULL set (m1+m2+m3+m4), not m1+m2; subsets are explicit opt-in via
--metric=. These tests pin the default in run.sh and in the docs that
describe it (source-grep style, no bash execution, no docker).
"""
import pathlib
import re
import unittest

HARNESS_DIR = pathlib.Path(__file__).resolve().parent
RUN_SH = HARNESS_DIR / "run.sh"
RUN_ALL_SH = HARNESS_DIR / "run-all.sh"
RUNBOOK_MD = HARNESS_DIR.parent / "runner" / "RUNBOOK.md"
README_MD = HARNESS_DIR.parent / "README.md"

FULL_SET = "m1+m2+m3+m4"


class MetricDefaultSourceOfTruthTest(unittest.TestCase):
    """The full-set default is anchored in run.sh and the docs."""

    def test_run_sh_defaults_to_full_metric_set(self):
        """run.sh anchors METRIC=m1+m2+m3+m4 as the default."""
        run_sh = RUN_SH.read_text()
        self.assertTrue(
            re.search(r"^METRIC=m1\+m2\+m3\+m4$", run_sh, re.MULTILINE),
            "run.sh must default METRIC to the full set (bd rc-awyoj)",
        )

    def test_run_sh_metric_explicit_wiring(self):
        """METRIC_EXPLICIT starts false and flips true on --metric=."""
        run_sh = RUN_SH.read_text()
        self.assertIn("METRIC_EXPLICIT=false", run_sh)
        metric_case_lines = [
            line for line in run_sh.splitlines() if "--metric=*)" in line
        ]
        self.assertTrue(metric_case_lines, "no --metric=*) case found")
        self.assertTrue(
            all("METRIC_EXPLICIT=true" in line for line in metric_case_lines),
            "the --metric=*) case must set METRIC_EXPLICIT=true",
        )

    def test_usage_states_full_set_default(self):
        """usage() names the full set as the default."""
        run_sh = RUN_SH.read_text()
        self.assertRegex(
            run_sh,
            r"default m1\+m2\+m3\+m4",
            "usage() must state the full-set default",
        )

    def test_docs_mention_full_set_default(self):
        """run-all.sh, RUNBOOK.md, README.md all name the full set."""
        for path in (RUN_ALL_SH, RUNBOOK_MD, README_MD):
            text = path.read_text()
            self.assertIn(
                FULL_SET, text,
                f"{path.name} must mention the full metric set",
            )

    def test_no_doc_claims_m1_m2_default(self):
        """No doc still claims the default metric set is m1+m2."""
        for path in (RUN_ALL_SH, RUNBOOK_MD, README_MD):
            text = path.read_text()
            self.assertNotRegex(
                text,
                r"default[^.\n]*m1\+m2(?!\+m3)",
                f"{path.name} still claims the m1+m2 default",
            )

    def test_m1_m2_remains_valid_subset(self):
        """The --metric validation still accepts m1+m2 as a subset."""
        run_sh = RUN_SH.read_text()
        self.assertRegex(
            run_sh,
            r"m1\|m2\|m1\+m2\|m3\|m3\+m4\|m1\+m2\+m3\+m4",
            "m1+m2 must remain a valid explicit subset",
        )


if __name__ == "__main__":
    unittest.main()