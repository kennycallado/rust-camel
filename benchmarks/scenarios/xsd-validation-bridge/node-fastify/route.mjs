// T4b xsd-validation-bridge fixture — node-fastify contender (bench-node
// task 3.1). Same protocol-B contract as ../node-native/route.mjs
// (see its header for the full extraction — env contract
// BENCH_PAYLOAD/BENCH_SCHEMA/BENCH_LATENCY_FILE, in-process
// xmllint-wasm validation exempt from the xml-bridge seam,
// timer:bench?period=10&repeatCount=10000 per-tick shape, startup
// self-test = wasm init slot, marker + latency + BENCH_XSD_TICK
// patterns), with the Fastify application booted in front:
// - Module import + `fastify()` construction + route registration +
//   `await app.ready()` run WITHOUT binding any socket — protocol B
//   has no wire protocol (the no-bind rule). `ready()` is the
//   load-bearing call (task 2.1 lesson): it drives the full avvio
//   boot that every co-contender pays before its marker (rust
//   ctx.start().await, Camel Main.run). The registered route is never
//   served.
// - Then the same timer route: per tick validate + latency record +
//   BENCH_XSD_TICK, marker `BENCH_ROUTE_READY <unix_ms>` exactly once
//   after the boot and self-test, then idle-until-killed.

import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import Fastify from "fastify";
import { validateXML } from "xmllint-wasm";

const fixtureDir = dirname(fileURLToPath(import.meta.url));

const payloadPath =
  process.env.BENCH_PAYLOAD ?? resolve(fixtureDir, "../shared/bench-payload.xml");
const schemaPath =
  process.env.BENCH_SCHEMA ?? resolve(fixtureDir, "../shared/schema.xsd");
const latencyFile =
  process.env.BENCH_LATENCY_FILE ??
  "/tmp/v3-protocol-b-xsd-validation-bridge_node-fastify.log";

// timer:bench?period=10&repeatCount=10000 — identical across all T4b
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
// marker — exactly once, AFTER the boot (ready()) and the self-test.
setTimeout(fireTick, PERIOD_MS);
console.log(`BENCH_ROUTE_READY ${Date.now()}`);

// repeatCount exhausted -> idle until killed, like ctrl_c().await.
setInterval(() => {}, 1 << 30);
