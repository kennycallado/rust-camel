// T4b xsd-validation-bridge fixture — node-native contender (bench-node
// task 3.1). First node-native fixture with a dependency: Node stdlib
// has no XML, so XSD validation lands on `xmllint-wasm` 5.3.0 (libxml2
// compiled to WebAssembly; pinned exactly in package.json — see the
// README for the engine-vs-Xerces-J auditability note).
//
// Contract (extracted from the existing contenders — rust-camel-lib
// main.rs, camel-standalone App.java, camel-quarkus BenchRoute.java):
// - Env contract: `BENCH_PAYLOAD` / `BENCH_SCHEMA` /
//   `BENCH_LATENCY_FILE` (the same names the rust fixture reads).
//   Defaults anchor to THIS fixture's location via import.meta.url
//   ("../shared/...") because the harness launches node cells with no
//   per-cell env and no cd — the rust fixture's CWD-relative defaults
//   would not resolve from the harness cwd. The harness protocol-B
//   probe reads latency records from the fixed cell path
//   /tmp/v3-protocol-b-<scenario>_<contender>.log, so that path is
//   the latency-file default.
// - EXEMPT from the compiled `xml-bridge` subprocess seam: the JVM
//   fixtures run Xerces-J in-process and the rust fixtures pay the
//   bridge tax; this contender validates IN-PROCESS against the SAME
//   byte-pinned shared assets (digest parity by construction).
// - Route shape: `timer:bench?period=10&repeatCount=10000` — per tick:
//   set_body(shared payload) -> XSD validate -> append
//   `BENCH_LATENCY <id> <duration_ns>` -> log `BENCH_XSD_TICK id=<n>`.
//   Ticks are sequential (setTimeout chain), like a Camel timer that
//   never overlaps route executions on its single consumer thread.
// - Marker: `BENCH_ROUTE_READY <unix_ms>` exactly once, after route
//   start (mirrors rust println after ctx.start() and the JVM
//   RouteStarted notifier).
// - Startup self-test BEFORE the marker: one full validation of the
//   shared payload; an invalid payload exits non-zero BEFORE the
//   marker (the abort-before-marker convention of the t2-json node
//   fixture's output assert). This call is also the wasm init slot:
//   the JVM compiles the Xerces schema once per process at route
//   start; node forces the wasm module fetch + compile + first schema
//   parse here so measured ticks pay only the per-call engine cost.
//   Placement rationale + the worker-per-call caveat: see README.

import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { validateXML } from "xmllint-wasm";

const fixtureDir = dirname(fileURLToPath(import.meta.url));

const payloadPath =
  process.env.BENCH_PAYLOAD ?? resolve(fixtureDir, "../shared/bench-payload.xml");
const schemaPath =
  process.env.BENCH_SCHEMA ?? resolve(fixtureDir, "../shared/schema.xsd");
const latencyFile =
  process.env.BENCH_LATENCY_FILE ??
  "/tmp/v3-protocol-b-xsd-validation-bridge_node-native.log";

// timer:bench?period=10&repeatCount=10000 — identical across all T4b
// fixtures.
const PERIOD_MS = 10;
const REPEAT_COUNT = 10000;

const payload = readFileSync(payloadPath, "utf8");
const schema = readFileSync(schemaPath, "utf8");

// Latency file: truncate at startup like every T4b fixture (JVM
// TRUNCATE_EXISTING write of "", rust File::create).
mkdirSync(dirname(latencyFile), { recursive: true });
writeFileSync(latencyFile, "");

// XSD validation, in-process (libxml2 compiled to wasm). The fileName
// labels are virtual — xmllint-wasm performs no IO; `contents` carry
// the bytes.
function validateBenchPayload() {
  return validateXML({
    xml: [{ fileName: "bench-payload.xml", contents: payload }],
    schema: [schema],
  });
}

function validationDetail(result) {
  return (
    (result.rawOutput && result.rawOutput.trim()) ||
    JSON.stringify(result.errors)
  );
}

// Startup self-test = the wasm init slot (JVM counterpart: Xerces
// schema compile at route start; node: module fetch + compile + first
// schema parse). Invalid payload -> non-zero exit BEFORE the marker.
const selfTest = await validateBenchPayload();
if (!selfTest.valid) {
  console.error(`error: xsd validation failed: ${validationDetail(selfTest)}`);
  process.exit(1);
}

// Per-tick work: validate -> append latency -> log tick. The timed
// span brackets ONLY the validation step, like the JVM BenchStart
// property set just before .to("validator:...") and read just after.
// A validation failure aborts the process non-zero, like a failing
// validator step erroring the JVM route.
let tick = 0;
function fireTick() {
  tick += 1;
  const t0 = process.hrtime.bigint();
  validateBenchPayload().then(
    (result) => {
      const durationNs = Number(process.hrtime.bigint() - t0);
      if (!result.valid) {
        console.error(
          `error: xsd validation failed on tick ${tick}: ${validationDetail(result)}`,
        );
        process.exit(1);
      }
      appendFileSync(latencyFile, `BENCH_LATENCY ${tick} ${durationNs}\n`);
      console.log(`BENCH_XSD_TICK id=${tick}`);
      if (tick < REPEAT_COUNT) {
        setTimeout(fireTick, PERIOD_MS);
      }
    },
    (err) => {
      console.error(`error: xsd validation threw on tick ${tick}: ${err}`);
      process.exit(1);
    },
  );
}

// Route start: first fire one period out (timer semantics), then the
// marker — exactly once.
setTimeout(fireTick, PERIOD_MS);
console.log(`BENCH_ROUTE_READY ${Date.now()}`);

// repeatCount exhausted -> idle until killed, like ctrl_c().await.
setInterval(() => {}, 1 << 30);
