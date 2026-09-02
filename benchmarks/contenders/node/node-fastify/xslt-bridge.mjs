// T4a xslt-bridge fixture — node-fastify contender (bench-node task
// 3.2). Same protocol-B contract as ../node-native/xslt-bridge.mjs (see its
// header for the full extraction — env contract
// BENCH_PAYLOAD/BENCH_STYLESHEET/BENCH_LATENCY_FILE, in-process
// saxon-js 2.7.0 transform exempt from the xml-bridge seam, xslt3
// SEF compile in the startup self-test slot, timer:bench?period=10&
// repeatCount=10000 per-tick shape, marker + latency +
// BENCH_XSLT_TICK + BENCH_XSLT_SELFTEST_SHA256 patterns), with the
// Fastify application booted in front:
// - Module import + `fastify()` construction + route registration +
//   `await app.ready()` run WITHOUT binding any socket — protocol B
//   has no wire protocol (the no-bind rule). `ready()` is the
//   load-bearing call (task 2.1 lesson): it drives the full avvio
//   boot that every co-contender pays before its marker (rust
//   ctx.start().await, Camel Main.run). The registered route is never
//   served.
// - Then the same timer route: per tick transform + latency record +
//   BENCH_XSLT_TICK, marker `BENCH_ROUTE_READY <unix_ms>` exactly
//   once after the boot and self-test, then idle-until-killed.

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
import Fastify from "fastify";
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
  "/tmp/v3-protocol-b-xslt-bridge_node-fastify.log";

// timer:bench?period=10&repeatCount=10000 — identical across all T4a
// fixtures.
const PERIOD_MS = 10;
const REPEAT_COUNT = 10000;

const app = Fastify();

// Registered before the boot so route compilation lands inside
// ready(), like a real application. It is never served: nothing is
// bound and this scenario has no request phase.
app.all("/bench", async () => "pong");

// Full avvio boot — plugin loading, route compilation, handler
// finalization — without binding any socket.
await app.ready();

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
// marker — exactly once, AFTER the boot (ready()) and the self-test.
setTimeout(fireTick, PERIOD_MS);
console.log(`BENCH_ROUTE_READY ${Date.now()}`);

// repeatCount exhausted -> idle until killed, like ctrl_c().await.
setInterval(() => {}, 1 << 30);
