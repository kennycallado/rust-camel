// T4a xslt-bridge fixture — node-native contender (bench-node task 3.2).
// Node stdlib has no XSLT engine, so the transform lands on `saxon-js`
// 2.7.0 (Saxon-JS: same vendor as the JVM's Saxon-HE but a DIFFERENT
// engine — Saxon-JS ≠ Saxon-HE; see README for the auditability note).
// The saxon-js runtime package executes only compiled stylesheets, so
// the sibling `xslt3` 2.7.0 CLI (same vendor) compiles the shared
// stylesheet to a SEF (stylesheet export file) at startup.
//
// Contract (extracted from the existing contenders — rust-camel-lib
// main.rs, camel-standalone App.java, camel-quarkus BenchRoute.java):
// - Env contract: `BENCH_PAYLOAD` / `BENCH_STYLESHEET` /
//   `BENCH_LATENCY_FILE` (the same names the rust fixture reads).
//   Defaults anchor to the scenario's shared data dir relative to
//   THIS fixture's location via import.meta.url — the fixture lives
//   in the consolidated runtime dir benchmarks/contenders/node/ while
//   the byte-pinned data stays in
//   benchmarks/scenarios/<scenario>/shared/ — because the harness
//   launches node cells with no per-cell env and no cd — the rust
//   fixture's CWD-relative defaults would not resolve from the
//   harness cwd. The harness protocol-B
//   probe reads latency records from the fixed cell path
//   /tmp/v3-protocol-b-<scenario>_<contender>.log, so that path is
//   the latency-file default.
// - EXEMPT from the compiled `xml-bridge` subprocess seam: the JVM
//   fixtures run Saxon-HE 12.5 in-process and the rust fixtures pay
//   the bridge tax; this contender transforms IN-PROCESS against the
//   SAME byte-pinned shared assets (same source bytes by
//   construction — no fixture-local XML/XSL copies).
// - Route shape: `timer:bench?period=10&repeatCount=10000` — per tick:
//   set_body(shared payload) -> XSLT transform -> append
//   `BENCH_LATENCY <id> <duration_ns>` -> log `BENCH_XSLT_TICK id=<n>`.
//   Ticks are sequential (setTimeout chain), like a Camel timer that
//   never overlaps route executions on its single consumer thread.
// - Marker: `BENCH_ROUTE_READY <unix_ms>` exactly once, after route
//   start (mirrors rust println after ctx.start() and the JVM
//   RouteStarted notifier).
// - Startup self-test BEFORE the marker: the xslt3 compile + one full
//   transform of the shared payload; a compile or transform failure
//   exits non-zero BEFORE the marker. This is the engine init slot:
//   the JVM compiles the stylesheet once per process at route start
//   (Camel Templates build); node forces the SEF compile + first
//   transform here so measured ticks pay only the per-call transform
//   cost (stylesheetInternal reuse — no per-tick recompile).
// - Output stability: the self-test logs the transform output digest
//   `BENCH_XSLT_SELFTEST_SHA256=<hex>` (pre-marker, startup-only —
//   never per tick). Cross-runtime byte-parity with Saxon-HE
//   serializers is NOT asserted (different engines may serialize
//   differently); the digest pins THIS fixture's output so drift in
//   engine version or asset bytes fails loudly against the committed
//   smoke evidence.

import { execFileSync } from "node:child_process";
import {
  appendFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import SaxonJS from "saxon-js";

const require = createRequire(import.meta.url);
const fixtureDir = dirname(fileURLToPath(import.meta.url));

const payloadPath =
  process.env.BENCH_PAYLOAD ??
  resolve(fixtureDir, "../../../scenarios/xslt-bridge/shared/bench-payload.xml");
const stylesheetPath =
  process.env.BENCH_STYLESHEET ??
  resolve(fixtureDir, "../../../scenarios/xslt-bridge/shared/identity-transform.xsl");
const latencyFile =
  process.env.BENCH_LATENCY_FILE ??
  "/tmp/v3-protocol-b-xslt-bridge_node-native.log";

// timer:bench?period=10&repeatCount=10000 — identical across all T4a
// fixtures.
const PERIOD_MS = 10;
const REPEAT_COUNT = 10000;

const payload = readFileSync(payloadPath, "utf8");

// Latency file: truncate at startup like every T4a fixture (JVM
// TRUNCATE_EXISTING write of "", rust File::create).
mkdirSync(dirname(latencyFile), { recursive: true });
writeFileSync(latencyFile, "");

// Engine init slot: compile the shared stylesheet to a SEF via the
// xslt3 CLI (same vendor; the saxon-js runtime has no compiler). JVM
// counterpart: Camel's XsltEndpoint builds the Templates object once
// at route start. The SEF is a throwaway temp file — the stylesheet
// source of truth stays under shared/.
const xslt3Bin = require.resolve("xslt3/xslt3.js");
const sefDir = mkdtempSync(join(tmpdir(), "t4a-sef-"));
const sefPath = join(sefDir, "identity-transform.sef.json");
execFileSync(process.execPath, [xslt3Bin, `-xsl:${stylesheetPath}`, `-export:${sefPath}`, "-nogo"], {
  stdio: "pipe",
});
const sef = JSON.parse(readFileSync(sefPath, "utf8"));
rmSync(sefDir, { recursive: true, force: true });

// XSLT transform, in-process (Saxon-JS 2.7.0, XSLT 3.0 engine — the
// shared stylesheet declares version="3.0"). stylesheetInternal is
// the pre-compiled SEF: no per-tick recompile, the counterpart of the
// JVM reusing one Templates object per call.
function transformBenchPayload() {
  return SaxonJS.transform(
    { stylesheetInternal: sef, sourceText: payload, destination: "serialized" },
    "sync",
  ).principalResult;
}

// Startup self-test: one full transform + output digest. Failure ->
// non-zero exit BEFORE the marker. This is also the first-touch warmup
// (JIT class loads, wasm/text codec init) so measured ticks pay only
// the steady-state transform cost.
let selfTestOutput;
try {
  selfTestOutput = transformBenchPayload();
} catch (err) {
  console.error(`error: xslt self-test transform failed: ${err}`);
  process.exit(1);
}
const selfTestDigest = createHash("sha256").update(selfTestOutput).digest("hex");
console.log(`BENCH_XSLT_SELFTEST_SHA256=${selfTestDigest}`);

// Per-tick work: transform -> append latency -> log tick. The timed
// span brackets ONLY the transform step, like the JVM BenchStart
// property set just before .to("xslt:...") and read just after. A
// transform failure aborts the process non-zero, like a failing xslt
// step erroring the JVM route.
let tick = 0;
function fireTick() {
  tick += 1;
  const t0 = process.hrtime.bigint();
  try {
    transformBenchPayload();
  } catch (err) {
    console.error(`error: xslt transform threw on tick ${tick}: ${err}`);
    process.exit(1);
  }
  const durationNs = Number(process.hrtime.bigint() - t0);
  appendFileSync(latencyFile, `BENCH_LATENCY ${tick} ${durationNs}\n`);
  console.log(`BENCH_XSLT_TICK id=${tick}`);
  if (tick < REPEAT_COUNT) {
    setTimeout(fireTick, PERIOD_MS);
  }
}

// Route start: first fire one period out (timer semantics), then the
// marker — exactly once, after the self-test.
setTimeout(fireTick, PERIOD_MS);
console.log(`BENCH_ROUTE_READY ${Date.now()}`);

// repeatCount exhausted -> idle until killed, like ctrl_c().await.
setInterval(() => {}, 1 << 30);
