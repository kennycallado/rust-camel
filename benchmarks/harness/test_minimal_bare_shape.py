"""Source-grep pins for the blessed minimal-bare http-server shape.

e_opus ruling D1 (2026-09-16, bd rc-h42s6): every http-server contender
route is EXACTLY `from(http://0.0.0.0:8080/bench) -> set_body("pong")`
— no per-request log lines, no counter, no trace path in the measured
route. rc-am22 proved this class of regression lands silently (a
"restore" reintroduced per-request work with a docstring claiming
family-wide symmetry that did not exist — audit contradiction C1).
These tests grep the fixture SOURCES so the shape cannot regress
without a red test. Binary-level proof is separate:
checks/trace-absent.sh (ruling item 1).

Conventions: stdlib unittest, source-grep only, no bash execution
(same style as test_metric_default.py).
"""
import pathlib
import re
import unittest

HARNESS_DIR = pathlib.Path(__file__).resolve().parent
BENCH = HARNESS_DIR.parent

LIB_HTTP = BENCH / "contenders" / "rust-camel-lib" / "src" / "scenarios" / "http-server.rs"
LIB_TRACE = BENCH / "contenders" / "rust-camel-lib" / "src" / "scenarios" / "http-server-trace.rs"
LIB_CARGO = BENCH / "contenders" / "rust-camel-lib" / "Cargo.toml"
AXUM_MAIN = BENCH / "contenders" / "axum-bare" / "src" / "main.rs"
NODE_NATIVE = BENCH / "contenders" / "node" / "node-native" / "http-server.mjs"
NODE_FASTIFY = BENCH / "contenders" / "node" / "node-fastify" / "http-server.mjs"
CLI_ROUTE = BENCH / "scenarios" / "http-server" / "rust-camel-cli" / "routes" / "http-server.yaml"
JAVA_DSL = (BENCH / "scenarios" / "http-server" / "camel-standalone" /
            "camel-standalone-dsl" / "src" / "main" / "java" / "com" / "rustcamel" / "bench" / "App.java")


class MinimalBareShapeTest(unittest.TestCase):
    """The measured http-server route is bare on every contender."""

    def test_lib_route_is_bare(self):
        """rust-camel-lib route: from -> route_id -> set_body, nothing else.

        The route-build region must contain no .log(/.process(/counter
        step; the ONLY println! in the file is the BENCH_ROUTE_READY
        marker. Trace code lives in the cfg-gated http-server-trace.rs.
        """
        src = LIB_HTTP.read_text()
        # Extract the route construction region (from RouteBuilder to .build()).
        m = re.search(r"RouteBuilder::from\(.*?\)\s*\.build\(\)\?", src, re.DOTALL)
        self.assertIsNotNone(m, "route builder region not found in http-server.rs")
        route = m.group(0)
        for banned in (".log(", ".process(", "AtomicU64", "fetch_add", "BENCH_HTTP_REQUEST"):
            self.assertNotIn(banned, route, f"banned per-request work '{banned}' in the measured route (e_opus D1)")
        # The marker is the only println! call in the file (docstring
        # mentions and eprintln! error paths are not stdout writes).
        real_printlns = [
            line for line in src.splitlines()
            if re.search(r"(?<!e)println!\(", line) and not line.lstrip().startswith("//")
        ]
        self.assertEqual(
            len(real_printlns), 1,
            f"exactly one println! (the marker) is allowed, got: {real_printlns}",
        )

    def test_lib_trace_is_cfg_gated_and_default_off(self):
        """Trace variant exists only behind the bench-trace feature."""
        self.assertTrue(LIB_TRACE.exists(), "trace module missing")
        cargo = LIB_CARGO.read_text()
        self.assertIn("bench-trace = []", cargo)
        # No default feature list may pull bench-trace in.
        self.assertNotIn("default = [\"bench-trace\"]", cargo)
        self.assertNotIn("default = ['bench-trace']", cargo)

    def test_node_and_axum_bare(self):
        """node-native/fastify + axum-bare: no per-request lines/counters."""
        for path in (NODE_NATIVE, NODE_FASTIFY, AXUM_MAIN):
            src = path.read_text()
            self.assertNotIn(
                "BENCH_HTTP_REQUEST", src,
                f"{path.name} still emits per-request trace lines (e_opus D1)",
            )
            for banned in ("requestId", "AtomicU64"):
                self.assertNotIn(banned, src, f"{path.name} still carries a counter ('{banned}')")

    def test_cli_and_java_still_bare(self):
        """cli route YAML + Java Pair A route remain the bare set_body shape."""
        cli = CLI_ROUTE.read_text()
        steps = re.search(r"steps:\n(.*)\Z", cli, re.DOTALL).group(1)
        self.assertNotIn("log", steps, "cli route gained a log step")
        self.assertIn("set_body", steps)

        java = JAVA_DSL.read_text()
        route = re.search(r"from\(\"jetty:[^\"]+\"\)(.*?);", java, re.DOTALL).group(1)
        self.assertNotIn(".log(", route)
        self.assertNotIn(".process(", route)
        self.assertIn("setBody", route)


if __name__ == "__main__":
    unittest.main()
