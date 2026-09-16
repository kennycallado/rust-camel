"""Tests for the native-image build cache (bd rc-wdy13).

The v2 harness used to pre-write the fingerprint on a cache miss,
BEFORE the build ran. A failed build then left the new fingerprint
paired with the old runner image, so the next run skipped the rebuild
and silently measured a stale binary. The fix moves the skip decision
(native_cache_plan), runner resolution (resolve_native_runner), and
the single fingerprint write path (write_native_fingerprint) into
lib/native_cache.sh, and the fingerprint advances ONLY after a
successful build + runner resolution.

These tests drive the lib via bash subprocesses over constructed
temp trees (no gradle, no docker, no network).
"""
import hashlib
import os
import pathlib
import subprocess
import tempfile
import unittest

HARNESS_DIR = pathlib.Path(__file__).resolve().parent
RUN_SH = HARNESS_DIR / "run.sh"
LIB_SH = HARNESS_DIR / "lib" / "native_cache.sh"

RUNNER_NAME = "foo-runner"
WRONG_DIGEST = "0" * 64


def _env():
    """Deterministic env for the docker-mode fingerprint manifest."""
    env = dict(os.environ)
    env.update({
        "BENCH_NATIVE_MODE": "docker",
        "JAVA_HOME": "/usr",
        "QUARKUS_NATIVE_BUILDER_IMAGE": "sha256:0000",
        # Docker mode must never invoke native-image; a regression in
        # the mode guard would hit this deterministic sentinel.
        "NATIVE_IMAGE_BIN": "/nonexistent-native-image",
    })
    return env


def _make_tree(root):
    """Construct a minimal native-subproject tree.

    Mirrors the inputs native_cache_plan hashes: native_dir with
    build.gradle.kts + application.properties, shared src/main with
    one file, gradle/ with one file, and a runner at
    <native_dir>/build/foo-runner. settings.gradle.kts and the JVM
    sibling's build.gradle.kts are absent on purpose — the plan must
    tolerate missing optional inputs.
    """
    cq = root / "camel-quarkus"
    native_dir = cq / "camel-quarkus-dsl-native"
    shared_src = cq / "src" / "main"
    gradle_dir = cq / "gradle"

    (shared_src / "java").mkdir(parents=True)
    (shared_src / "java" / "Route.java").write_text("class Route {}\n")
    gradle_dir.mkdir(parents=True)
    (gradle_dir / "gradle.properties").write_text("org.gradle.caching=true\n")
    (native_dir / "src" / "main" / "resources").mkdir(parents=True)
    (native_dir / "build.gradle.kts").write_text("plugins { }\n")
    app_props = native_dir / "src" / "main" / "resources" / "application.properties"
    app_props.write_text("quarkus.native.resources.includes=camel/routes.yaml\n")
    build_dir = native_dir / "build"
    build_dir.mkdir()
    runner = build_dir / RUNNER_NAME
    runner.write_text("#!/bin/sh\n")

    return {
        "native_dir": native_dir,
        "shared_src": shared_src,
        "sibling": cq / "src" / "build.gradle.kts",   # absent (optional)
        "settings": cq / "settings.gradle.kts",        # absent (optional)
        "gradle": gradle_dir,
        "app_props": app_props,
        "runner": runner,
        "runner_glob": f"{build_dir}/*-runner",
    }


_PLAN_TMPL = (
    'source "{lib}"\n'
    'native_cache_plan "{native_dir}" "{shared_src}" "{sibling}" '
    '"{settings}" "{gradle}" "{app_props}" "{runner_glob}"\n'
    'plan_rc=$?\n'
    'printf \'FINGERPRINT=%s\\n\' "$NATIVE_CACHE_FINGERPRINT"\n'
    'printf \'RUNNER=%s\\n\' "$NATIVE_CACHE_RUNNER"\n'
    "exit $plan_rc\n"
)


class NativeCachePlanTest(unittest.TestCase):
    """native_cache_plan is fail-closed and never writes."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tree = _make_tree(pathlib.Path(self._tmp.name))
        self.fp_file = self.tree["native_dir"] / ".bench-fingerprint"

    def _plan(self):
        script = _PLAN_TMPL.format(lib=LIB_SH, **self.tree)
        return subprocess.run(
            ["bash", "-c", script], capture_output=True, text=True,
            env=_env(),
        )

    def _capture_fingerprint(self, proc):
        for line in proc.stdout.splitlines():
            if line.startswith("FINGERPRINT="):
                return line[len("FINGERPRINT="):]
        self.fail(f"fingerprint not captured: {proc.stdout!r}")

    def test_stale_fingerprint_present_runner_rebuilds(self):
        """bd AC: stale fingerprint + present image fails closed.

        The pre-fix harness skipped the build here (fingerprint had
        been pre-advanced by a failed run) and measured the stale
        binary. The plan must demand a rebuild.
        """
        self.fp_file.write_text(WRONG_DIGEST)

        proc = self._plan()

        self.assertEqual(proc.returncode, 1, "stale fingerprint must BUILD")
        # The plan must not touch the stored file (no pre-write).
        self.assertEqual(self.fp_file.read_text(), WRONG_DIGEST)

    def test_match_present_runner_skips(self):
        """Exact-match fingerprint + resolvable runner ⇒ SKIP."""
        first = self._plan()                       # BUILD: capture digest
        self.assertEqual(first.returncode, 1)
        self.fp_file.write_text(self._capture_fingerprint(first))

        proc = self._plan()

        self.assertEqual(proc.returncode, 0, "match + runner must SKIP")
        self.assertIn(f"RUNNER={self.tree['runner']}", proc.stdout)

    def test_match_missing_runner_builds(self):
        """Fingerprint match but no runner ⇒ BUILD (partial state)."""
        first = self._plan()
        self.fp_file.write_text(self._capture_fingerprint(first))
        self.tree["runner"].unlink()

        proc = self._plan()

        self.assertEqual(proc.returncode, 1, "missing runner must BUILD")

    def test_first_ever_builds(self):
        """No fingerprint file at all ⇒ BUILD, and plan writes nothing."""
        self.assertFalse(self.fp_file.exists())

        proc = self._plan()

        self.assertEqual(proc.returncode, 1, "first-ever build must BUILD")
        self.assertFalse(
            self.fp_file.exists(),
            "plan must not pre-write the fingerprint file",
        )

    def test_manifest_golden_layout(self):
        """The docker-mode manifest layout is pinned by a Python mirror.

        Existing .bench-fingerprint files only keep skipping when the
        manifest stays byte-identical. Any header line added/reordered,
        separator changed, or section dropped alters every digest and
        silently invalidates all caches. This mirror fails loudly on
        such drift. If the manifest is changed ON PURPOSE, update the
        mirror in the same commit and accept the one-time rebuilds.

        NOTE the inherited quirk this pins: the 7 section headers emit
        a LITERAL backslash-n (`$'\\n'` in bash — single-quoted
        backslash), while the env header lines emit a real newline.
        The quirk predates the lib extraction (run.sh had the same 7);
        byte-identity with previously stored fingerprints is
        load-bearing, so the mirror reproduces it exactly.
        """
        tree = self.tree
        def sha(path):
            return hashlib.sha256(path.read_bytes()).hexdigest()

        manifest = (
            f"=== JAVA_HOME === /usr\n"
            f"=== build mode === docker\n"
            f"=== builder image === sha256:0000\n"
            f"=== native-image --version === container (skipped)\n"
            f"=== shared src/main ===\\n"
            f"{tree['shared_src'] / 'java' / 'Route.java'}\t"
            f"{sha(tree['shared_src'] / 'java' / 'Route.java')}\n"
            f"=== native build.gradle.kts ===\\n"
            f"{sha(tree['native_dir'] / 'build.gradle.kts')}\n"
            # settings.gradle.kts + sibling build.gradle.kts absent in
            # the tree -> sections skipped entirely.
            f"=== application.properties ===\\n"
            f"{sha(tree['app_props'])}\n"
            f"=== gradle/ ===\\n"
            f"{tree['gradle'] / 'gradle.properties'}\t"
            f"{sha(tree['gradle'] / 'gradle.properties')}\n"
        )
        expected = hashlib.sha256(manifest.encode()).hexdigest()

        proc = self._plan()

        self.assertEqual(proc.returncode, 1)
        self.assertEqual(
            self._capture_fingerprint(proc), expected,
            "fingerprint manifest drifted from the pinned layout — "
            "every existing cache digest changes; see the docstring",
        )

    def test_resolve_runner_first_match_wins(self):
        """Multi-match glob resolves to the first (C-collation) match.

        The old resolvers used `ls $glob | head -1`; resolve_native_runner
        must keep picking the same file (run.sh exports LC_ALL=C at
        startup; the test pins C explicitly so it is locale-independent).
        """
        build_dir = self.tree["native_dir"] / "build"
        (build_dir / "zzz-runner").write_text("#!/bin/sh\n")

        script = 'source "{lib}"\nresolve_native_runner "{glob}"\n'.format(
            lib=LIB_SH, glob=self.tree["runner_glob"],
        )
        env = _env()
        env["LC_ALL"] = "C"
        proc = subprocess.run(
            ["bash", "-c", script], capture_output=True, text=True, env=env,
        )

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stdout, str(build_dir / RUNNER_NAME))

    def test_write_only_after_success_lifecycle(self):
        """A failed build never advances the fingerprint.

        (a) plan says BUILD and a failing build writes nothing — the
            old (wrong) content survives, so the next plan still says
            BUILD. (b) after a simulated successful build + runner
            resolution, write_native_fingerprint records the digest
            and the next plan says SKIP.
        """
        self.fp_file.write_text(WRONG_DIGEST)
        script = (
            'source "{lib}"\n'
            'native_cache_plan "{native_dir}" "{shared_src}" "{sibling}" '
            '"{settings}" "{gradle}" "{app_props}" "{runner_glob}"\n'
            'echo "RC1=$?"\n'
            '[ "$(cat "{fp_file}")" = "{old}" ] '
            '&& echo "OLD_PRESERVED=yes" || echo "OLD_PRESERVED=no"\n'
            '# (a) failed build: nothing written.\n'
            '# (b) successful build + runner resolution:\n'
            'write_native_fingerprint "{native_dir}"\n'
            'native_cache_plan "{native_dir}" "{shared_src}" "{sibling}" '
            '"{settings}" "{gradle}" "{app_props}" "{runner_glob}"\n'
            'echo "RC2=$?"\n'
        ).format(lib=LIB_SH, fp_file=self.fp_file, old=WRONG_DIGEST,
                 **self.tree)

        proc = subprocess.run(
            ["bash", "-c", script], capture_output=True, text=True,
            env=_env(),
        )

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("RC1=1", proc.stdout)      # build needed
        self.assertIn("OLD_PRESERVED=yes", proc.stdout)  # failed build wrote nothing
        self.assertIn("RC2=0", proc.stdout)      # only success advances the cache


class RunShUnifiedSourceOfTruthTest(unittest.TestCase):
    """run.sh delegates skip decision, resolution, and writes to the lib."""

    def test_fingerprint_write_lives_only_in_lib(self):
        """No code path in run.sh writes .bench-fingerprint directly."""
        run_sh = RUN_SH.read_text()
        for lineno, line in enumerate(run_sh.splitlines(), start=1):
            if ".bench-fingerprint" in line:
                self.assertNotRegex(
                    line, r">\s*\"?[^&|;]*\.bench-fingerprint",
                    f"run.sh:{lineno} writes the fingerprint file — "
                    "writes belong to lib/native_cache.sh",
                )
        # No fp_file-style indirection hiding a write either.
        self.assertNotIn("fp_file", run_sh)
        # The lib owns the single write path...
        lib_sh = LIB_SH.read_text()
        self.assertRegex(
            lib_sh, r"printf '%s' \"\$NATIVE_CACHE_FINGERPRINT\" >",
        )
        # ...and run.sh reaches it only through write_native_fingerprint.
        self.assertIn('write_native_fingerprint "$native_dir"', run_sh)

    def test_resolvers_use_shared_function(self):
        """Both resolver sites call resolve_native_runner."""
        run_sh = RUN_SH.read_text()
        self.assertIn('resolve_native_runner "$q_dsl_native_glob"', run_sh)
        self.assertIn('resolve_native_runner "$q_yaml_native_glob"', run_sh)
        self.assertNotIn("ls $q_dsl_native_glob", run_sh)
        self.assertNotIn("ls $q_yaml_native_glob", run_sh)
        # Skip-message drift guard (stable string, grepped nowhere else).
        self.assertIn(
            "fingerprint match + runner present, skipping build for "
            "$subproject", run_sh,
        )

    def test_cross_reference_comments(self):
        """rc-wdy13 cross-refs name the shared lib at the key sites."""
        run_sh = RUN_SH.read_text()
        self.assertGreaterEqual(
            run_sh.count("rc-wdy13"), 3,
            "build_native_artifact + both resolvers must cross-reference "
            "rc-wdy13",
        )
        lib_sh = LIB_SH.read_text()
        self.assertIn("rc-wdy13", lib_sh)
        self.assertIn("lib/native_cache.sh", run_sh)


if __name__ == "__main__":
    unittest.main()
