"""Regression tests for run.sh scenario auto-discovery (bd rc-dh7t).

Auto-discovery (bare `bench run --dry-run`, no --scenarios=) must SKIP
tracked-but-inactive scenario dirs — ones with no SCENARIO_MARKER
registered, e.g. multi-step — with a stderr notice, never abort.
The fail-loud marker check for EXPLICIT --scenarios= selection
(resolve_all_cells) is intentional and not covered here: reaching it
requires the full fixture wiring.

These tests execute the REAL discover_scenarios() body, extracted
verbatim from run.sh, against a synthetic SCENARIOS_DIR. The full
--dry-run path needs built fixtures and bridge binaries, so it is not
unit-testable in CI; discovery itself has no such prerequisite.
"""
import pathlib
import re
import shlex
import subprocess
import tempfile
import unittest

RUN_SH = pathlib.Path(__file__).resolve().parent / "run.sh"

SKIP_NOTICE = (
    "notice: skipping inactive scenario 'multi-step'"
    " (no marker registered)"
)


def discover_body():
    """Extract the discover_scenarios() function verbatim from run.sh."""
    match = re.search(
        r"^discover_scenarios\(\) \{.*?^\}$",
        RUN_SH.read_text(),
        re.S | re.M,
    )
    if match is None:
        raise AssertionError("discover_scenarios() not found in run.sh")
    return match.group(0)


class DiscoverScenariosTest(unittest.TestCase):
    """discover_scenarios() over a synthetic scenarios dir."""

    def run_discovery(self, scenario_names):
        """Run the extracted function against a temp scenarios dir.

        One active scenario (t2-json) gets a registered marker, as the
        real harness does; every other dir stays marker-less, matching
        the tracked-but-inactive multi-step situation.
        """
        with tempfile.TemporaryDirectory() as tmp:
            scenarios_dir = pathlib.Path(tmp) / "scenarios"
            for name in scenario_names:
                (scenarios_dir / name).mkdir(parents=True)
            script = "\n".join([
                "set -u",
                'declare -A SCENARIO_MARKER=(["t2-json"]='
                '"BENCH_ROUTE_READY bytes=45")',
                'SCENARIOS_FILTER=""',
                f"SCENARIOS_DIR={shlex.quote(str(scenarios_dir))}",
                discover_body(),
                "discover_scenarios",
                'printf \'RESOLVED=%s\\n\' "${SCENARIOS[*]}"',
                "",
            ])
            return subprocess.run(
                ["bash", "-c", script],
                capture_output=True,
                text=True,
                timeout=30,
            )

    def test_auto_discovery_skips_inactive_multi_step(self):
        """Inactive multi-step dir yields a notice, not an abort."""
        proc = self.run_discovery(["multi-step", "t2-json"])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn(SKIP_NOTICE, proc.stderr)
        self.assertIn("RESOLVED=t2-json", proc.stdout)
        self.assertNotIn("multi-step", proc.stdout)

    def test_auto_discovery_all_inactive_fails_loud(self):
        """Skip must not silently produce an empty scenario matrix."""
        proc = self.run_discovery(["multi-step"])
        self.assertEqual(proc.returncode, 1)
        self.assertIn(SKIP_NOTICE, proc.stderr)
        self.assertIn("error: no scenarios found under", proc.stderr)


if __name__ == "__main__":
    unittest.main()
