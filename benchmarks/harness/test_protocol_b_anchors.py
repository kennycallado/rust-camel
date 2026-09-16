"""Source-grep pins for the e_opus D3 Protocol-B window anchor set.

e_opus ruling D3 (2026-09-16, bd rc-h42s6; see also the
bench_instrument module doc in crates/camel-cli/src/commands/
bench_instrument.rs) makes the Java/lib anchor set the single
normative Protocol-B window definition for EVERY protocol-B cell:

    [body supply (EXCLUDED)] -> WINDOW START -> [core pipeline] ->
    WINDOW CLOSE (the BENCH_LATENCY append) -> [assert / header /
    marker (EXCLUDED)] -> [trailing log (EXCLUDED)]

D4 keeps the t2-json full semantic re-parse strictly OUTSIDE the
window. These tests grep the fixture SOURCES so no cell can drift
back to a wider or narrower window without a red test (same
source-grep style as test_minimal_bare_shape.py).

Cell-to-file notes: for the two JVM yaml cells per scenario the step
ORDER lives in the routes.yaml (the beans are position-independent),
so those cells are pinned via their yaml route files.
"""
import pathlib
import re
import unittest

HARNESS_DIR = pathlib.Path(__file__).resolve().parent
BENCH = HARNESS_DIR.parent
SCEN = BENCH / "scenarios"
LIB_SCEN = BENCH / "contenders" / "rust-camel-lib" / "src" / "scenarios"
NODE = BENCH / "contenders" / "node"

# t2-json
T2J_JAVA_DSL = SCEN / "t2-json" / "camel-standalone" / "camel-standalone-dsl" / "src" / "main" / "java" / "com" / "rustcamel" / "bench" / "App.java"
T2J_QUARKUS_DSL = SCEN / "t2-json" / "camel-quarkus" / "camel-quarkus-dsl" / "src" / "main" / "java" / "com" / "rustcamel" / "bench" / "BenchRoute.java"
T2J_SYAML = SCEN / "t2-json" / "camel-standalone" / "camel-standalone-yaml" / "src" / "main" / "resources" / "routes.yaml"
T2J_QYAML = SCEN / "t2-json" / "camel-quarkus" / "camel-quarkus-yaml" / "src" / "main" / "resources" / "camel" / "routes.yaml"
T2J_LIB = LIB_SCEN / "t2-json.rs"
T2J_NODE_NATIVE = NODE / "node-native" / "t2-json.mjs"
T2J_NODE_FASTIFY = NODE / "node-fastify" / "t2-json.mjs"
T2J_CLI = SCEN / "t2-json" / "rust-camel-cli" / "routes" / "t2-json.yaml"

# t2-realistic-eip
T2R_JAVA_DSL = SCEN / "t2-realistic-eip" / "camel-standalone" / "camel-standalone-dsl" / "src" / "main" / "java" / "com" / "rustcamel" / "bench" / "App.java"
T2R_QUARKUS_DSL = SCEN / "t2-realistic-eip" / "camel-quarkus" / "camel-quarkus-dsl" / "src" / "main" / "java" / "com" / "rustcamel" / "bench" / "BenchRoute.java"
T2R_SYAML = SCEN / "t2-realistic-eip" / "camel-standalone" / "camel-standalone-yaml" / "src" / "main" / "resources" / "routes.yaml"
T2R_QYAML = SCEN / "t2-realistic-eip" / "camel-quarkus" / "camel-quarkus-yaml" / "src" / "main" / "resources" / "camel" / "routes.yaml"
T2R_LIB = LIB_SCEN / "t2-realistic-eip.rs"
T2R_NODE_NATIVE = NODE / "node-native" / "t2-realistic-eip.mjs"
T2R_NODE_FASTIFY = NODE / "node-fastify" / "t2-realistic-eip.mjs"
T2R_CLI = SCEN / "t2-realistic-eip" / "rust-camel-cli" / "routes" / "t2-realistic-eip.yaml"

# split-aggregate
SPLIT_LIB = LIB_SCEN / "split-aggregate.rs"
SPLIT_NODE_NATIVE = NODE / "node-native" / "split-aggregate.mjs"
SPLIT_NODE_FASTIFY = NODE / "node-fastify" / "split-aggregate.mjs"
SPLIT_CLI = SCEN / "split-aggregate" / "rust-camel-cli" / "routes" / "split-aggregate.yaml"

# bridges (cli-only changes; wrappers + yamls)
XSD_WRAPPER = SCEN / "xsd-validation-bridge" / "rust-camel-cli" / "xsd-validation-bridge-cli-wrapper.sh"
XSLT_WRAPPER = SCEN / "xslt-bridge" / "rust-camel-cli" / "xslt-bridge-cli-wrapper.sh"
XSD_CLI_YAML = SCEN / "xsd-validation-bridge" / "rust-camel-cli" / "routes" / "xsd-bench.yaml"
XSLT_CLI_YAML = SCEN / "xslt-bridge" / "rust-camel-cli" / "routes" / "xslt-bench.yaml"

SENTINEL_START = 'log: "BENCH_WINDOW_START"'
SENTINEL_END = 'log: "BENCH_WINDOW_END"'


class AnchorTestCase(unittest.TestCase):
    """Shared helper: first-occurrence index ordering over source text."""

    def assert_ordered(self, text, markers, label):
        prev = -1
        for marker in markers:
            idx = text.find(marker)
            self.assertGreaterEqual(
                idx, 0, f"{label}: marker not found in source: {marker!r}")
            self.assertGreater(
                idx, prev,
                f"{label}: marker {marker!r} (idx {idx}) must come AFTER the "
                f"previous marker (idx {prev}) — the D3 anchor order regressed")
            prev = idx

    def assert_count(self, text, marker, expected, label):
        got = text.count(marker)
        self.assertEqual(
            got, expected,
            f"{label}: expected {expected} occurrence(s) of {marker!r}, got {got}")

    def assert_sentinels_top_level(self, text, label):
        """Both window sentinels sit at the SAME top-level indentation.

        Flat-text ordering cannot distinguish a BENCH_WINDOW_END
        nested inside a container step (deeper YAML indentation) from
        one at the top level — the module rejects nested sentinels
        (bench_instrument count_sentinels_anywhere), so the fixture
        must never carry one, and this pins the top-level placement:
        both sentinel lines share an identical leading-indent column.
        """
        indents = {}
        for sentinel in (SENTINEL_START, SENTINEL_END):
            lines = [ln for ln in text.splitlines() if sentinel in ln]
            self.assertEqual(
                len(lines), 1,
                f"{label}: expected exactly one line carrying {sentinel!r}, "
                f"got {len(lines)}")
            indents[sentinel] = len(lines[0]) - len(lines[0].lstrip())
        self.assertEqual(
            indents[SENTINEL_START], indents[SENTINEL_END],
            f"{label}: sentinel indentations differ "
            f"({indents[SENTINEL_START]} vs {indents[SENTINEL_END]}) — "
            "a sentinel is nested inside a container step (e_opus D3 "
            "requires top-level placement)")


class T2JsonAnchorsTest(AnchorTestCase):
    """t2-json: set_body -> stamp -> unmarshal -> filter -> insert ->
    marshal -> LATENCY APPEND (close) -> assert (+ benchOutLen header)
    -> marker."""

    def test_java_dsl_pair_a(self):
        for path in (T2J_JAVA_DSL, T2J_QUARKUS_DSL):
            src = path.read_text()
            self.assert_ordered(src, [
                ".setBody(constant(payload))",
                'exchange.setProperty("BenchStart"',
                ".marshal().json(JsonLibrary.Jackson)",
                'line = "BENCH_LATENCY',
                ".process(assertOutput(size))",
                '"BENCH_ROUTE_READY bytes=',
            ], f"t2-json/{path.name}")

    def test_java_yaml_cells(self):
        for path in (T2J_SYAML, T2J_QYAML):
            src = path.read_text()
            self.assert_ordered(src, [
                'ref: "benchBody"',
                'ref: "markStart"',
                "- marshal:",
                'ref: "writeLatency"',
                'ref: "assertOutput"',
                'ref: "emitMarker"',
            ], f"t2-json/{path.name}")

    def test_lib(self):
        src = T2J_LIB.read_text()
        self.assert_ordered(src, [
            ".set_body(body)",
            "exchange.set_extension(BENCH_START",
            '.marshal("json")?',
            'format!("BENCH_LATENCY',
            "assert_bench_output(size, &text)",
            'set_header("benchOutLen"',
            'tracing::info!("BENCH_ROUTE_READY',
        ], "t2-json/t2-json.rs")
        # F1 alignment: the lib cell sets the benchOutLen header (post-window).
        self.assertIn('set_header("benchOutLen"', src)
        # Timer carries delay=0 (first tick fires immediately, like the peers).
        self.assertIn(
            'RouteBuilder::from("timer:bench?period=10&repeatCount=10000&delay=0")',
            src, "t2-json/t2-json.rs: lib timer lacks delay=0")

    def test_node_family(self):
        for path in (T2J_NODE_NATIVE, T2J_NODE_FASTIFY):
            src = path.read_text()
            self.assert_ordered(src, [
                "const body = tickBody;",
                "const t0 = process.hrtime.bigint();",
                "runTickPipeline(body);",
                "appendFileSync(latencyFile",
                "len = assertBenchOutput(size, out);",
                "emitReadyMarker(len);",
            ], f"t2-json/{path.name}")

    def test_cli_sentinels_and_d4_assert(self):
        src = T2J_CLI.read_text()
        self.assert_count(src, SENTINEL_START, 1, "t2-json cli")
        self.assert_count(src, SENTINEL_END, 1, "t2-json cli")
        self.assert_sentinels_top_level(src, "t2-json cli")
        # START after the LAST body-supply step (cache latch + input
        # assert + digest log), END before the post-window steps.
        self.assert_ordered(src, [
            "- cache:",
            "BENCH_INPUT_SHA256=GOLDEN",
            SENTINEL_START,
            "- unmarshal: json",
            SENTINEL_END,
            "JSON.parse(camel.body)",
            'key: "benchOutLen"',
            "bench-marker-once",
        ], "t2-json cli")
        # D4: the assert is a FULL semantic re-parse, not len+contains.
        for semantic in ("out.id !== \"bench\"", '"seq" in out',
                         "/^b+$/.test(out.fill)", "out.bench !== true"):
            self.assertIn(semantic, src,
                          f"t2-json cli D4 assert lost semantic check: {semantic}")


class T2RealisticEipAnchorsTest(AnchorTestCase):
    """t2r: set_body -> stamp -> set_header -> filter -> choice ->
    LATENCY APPEND (close) -> marker gate + log(body=...)."""

    def test_java_dsl_pair_a(self):
        for path in (T2R_JAVA_DSL, T2R_QUARKUS_DSL):
            src = path.read_text()
            self.assert_ordered(src, [
                '.setBody(constant("ping"))',
                'exchange.setProperty("BenchStart"',
                'line = "BENCH_LATENCY',
                '"BENCH_ROUTE_READY body=',
            ], f"t2r/{path.name}")

    def test_java_yaml_cells(self):
        for path in (T2R_SYAML, T2R_QYAML):
            src = path.read_text()
            self.assert_ordered(src, [
                'constant: "ping"',
                'ref: "markStart"',
                'ref: "writeLatency"',
                'ref: "emitMarker"',
            ], f"t2r/{path.name}")

    def test_lib(self):
        src = T2R_LIB.read_text()
        self.assert_ordered(src, [
            '.set_body("ping")',
            "exchange.set_extension(BENCH_START",
            'format!("BENCH_LATENCY',
            'tracing::info!("BENCH_ROUTE_READY',
        ], "t2r/t2-realistic-eip.rs")
        # Single marker line (the `body=` form only — audit F1 fix).
        self.assert_count(
            src, 'tracing::info!("BENCH_ROUTE_READY', 1, "t2r lib marker lines")
        self.assertIn(
            'RouteBuilder::from("timer:bench?period=10&repeatCount=10000&delay=0")',
            src, "t2r: lib timer lacks delay=0")

    def test_node_family(self):
        for path in (T2R_NODE_NATIVE, T2R_NODE_FASTIFY):
            src = path.read_text()
            self.assert_ordered(src, [
                'const ex = { body: "ping", headers: {} };',
                "const t0 = process.hrtime.bigint();",
                "runTickPipeline(ex);",
                "appendFileSync(latencyFile",
                "logStep(ex);",
            ], f"t2r/{path.name}")

    def test_cli_sentinels(self):
        src = T2R_CLI.read_text()
        self.assert_count(src, SENTINEL_START, 1, "t2r cli")
        self.assert_count(src, SENTINEL_END, 1, "t2r cli")
        self.assert_sentinels_top_level(src, "t2r cli")
        self.assert_ordered(src, [
            "- set_body:",
            SENTINEL_START,
            "- set_header:",
            'value: "pong-other"',
            SENTINEL_END,
            "bench-marker-once",
        ], "t2r cli")


class SplitAggregateAnchorsTest(AnchorTestCase):
    """split: set_body [+ cli supply validation] -> stamp -> unmarshal
    -> split -> to(direct) -> LATENCY APPEND (close, LAST step of the
    timer route). The agg route stays INSIDE the window via the
    synchronous direct dispatch — no sentinels there."""

    def test_lib(self):
        src = SPLIT_LIB.read_text()
        self.assert_ordered(src, [
            ".set_body(array)",
            "= Instant::now();",
            ".split(SplitterConfig",
            'format!("BENCH_LATENCY',
        ], "split/split-aggregate.rs")
        # Monotonic window clock (audit clock note): the SystemTime
        # API call is gone (doc mentions of the old clock are fine).
        self.assertNotIn("SystemTime::now", src,
                         "split lib window clock regressed to non-monotonic SystemTime")
        self.assertIn(
            'RouteBuilder::from("timer:bench?period=10&repeatCount=10000&delay=0")',
            src, "split: lib timer lacks delay=0")

    def test_node_family(self):
        for path in (SPLIT_NODE_NATIVE, SPLIT_NODE_FASTIFY):
            src = path.read_text()
            self.assert_ordered(src, [
                "const array = tickBody;",
                "const t0 = process.hrtime.bigint();",
                "runTickPipeline(array).then(",
                "appendFileSync(latencyFile",
            ], f"split/{path.name}")

    def test_cli_sentinels_and_f2_assert(self):
        src = SPLIT_CLI.read_text()
        self.assert_count(src, SENTINEL_START, 1, "split cli")
        self.assert_count(src, SENTINEL_END, 1, "split cli")
        self.assert_sentinels_top_level(src, "split cli")
        # START after the supply validation, END after the split.
        self.assert_ordered(src, [
            "- set_body:",
            "body.len() != 591",
            "bench-sha-once",
            SENTINEL_START,
            "- unmarshal: json",
            "- split:",
            SENTINEL_END,
        ], "split cli")
        # END is the LAST step of the timer route: no further step
        # entry may sit between it and the agg route definition.
        timer_region = src[src.find('id: "bench-split-route"'):
                           src.find('  - id: "bench-agg-route"')]
        tail = timer_region[timer_region.rfind(SENTINEL_END) + len(SENTINEL_END):]
        self.assertIsNone(
            re.search(r"^\s*-\s", tail, re.MULTILINE),
            "split cli: a step follows BENCH_WINDOW_END in the timer route "
            "(END must be the last step)")
        # The agg route (direct consumer, inside the window) carries no
        # sentinels — route mode passes it through UNINSTRUMENTED.
        agg_region = src[src.find('id: "bench-agg-route"'):]
        for sentinel in (SENTINEL_START, SENTINEL_END):
            self.assertNotIn(sentinel, agg_region,
                             "split cli agg route must not carry sentinels")
        # F2: Java/lib consistency check — reported CamelAggregatedSize
        # vs actual collected length; property stamped with the ACTUAL.
        self.assertIn('camel.property("CamelAggregatedSize")', agg_region,
                      "split cli F2: consistency check lost the reported-size read")
        self.assertIn("reported !== actual", agg_region,
                      "split cli F2: reported-vs-actual consistency check missing")
        self.assertIn('camel.set_property("bench.aggregated.size"', agg_region,
                      "split cli F2: bench.aggregated.size no longer stamped in-route")
        self.assertNotIn("value: 100", src,
                         "split cli F2: hardcoded set_property value 100 is back")


class BridgeAnchorsTest(AnchorTestCase):
    """Bridges stay in the module's default PAIR mode: the wrapper must
    NOT select route mode, and the bridge yamls carry no sentinels —
    pair mode brackets exactly the top-level to(validator/xslt) step,
    which IS the blessed anchor set for these cells."""

    def test_wrappers_have_no_route_mode(self):
        for path in (XSD_WRAPPER, XSLT_WRAPPER):
            src = path.read_text()
            self.assertNotIn(
                "BENCH_LATENCY_MODE=route", src,
                f"{path.name}: bridge wrapper must not export route mode "
                "(pair mode brackets the top-level to() step — e_opus D3)")
            self.assertIn(
                "export BENCH_LATENCY_FILE=", src,
                f"{path.name}: latency-file export must stay intact")

    def test_bridge_yamls_have_no_sentinels(self):
        for path in (XSD_CLI_YAML, XSLT_CLI_YAML):
            src = path.read_text()
            self.assertNotIn(
                "BENCH_WINDOW", src,
                f"{path.name}: pair-mode bridge yaml must carry no sentinels")
            self.assertIsNotNone(
                re.search(r'^\s+- to: "(validator|xslt):', src, re.MULTILINE),
                f"{path.name}: top-level to(validator/xslt) step missing")


class LibTimerDelayTest(AnchorTestCase):
    """All three tick-scenario lib timers fire their first tick
    immediately (delay=0) — the seven peers' shape (audit timer note)."""

    def test_lib_timers_carry_delay_zero(self):
        for path in (T2J_LIB, T2R_LIB, SPLIT_LIB):
            src = path.read_text()
            self.assertIn(
                'RouteBuilder::from("timer:bench?period=10&repeatCount=10000&delay=0")',
                src, f"{path.name}: lib timer lacks delay=0")


if __name__ == "__main__":
    unittest.main()
