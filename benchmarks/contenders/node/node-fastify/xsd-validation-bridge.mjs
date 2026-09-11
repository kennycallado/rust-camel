// T4b xsd-validation-bridge fixture — node-fastify contender (bench-node
// task 3.1). Same protocol-B contract as ../node-native/xsd-validation-bridge.mjs
// (see its header for the full extraction — env contract
// BENCH_PAYLOAD/BENCH_SCHEMA/BENCH_LATENCY_FILE, in-process
// xmllint-wasm validation exempt from the xml-bridge seam and the
// persistent-worker engine init placement (rc-audm.1),
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
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Worker } from "node:worker_threads";
import Fastify from "fastify";

const fixtureDir = dirname(fileURLToPath(import.meta.url));

const payloadPath =
  process.env.BENCH_PAYLOAD ??
  resolve(fixtureDir, "../../../scenarios/xsd-validation-bridge/shared/bench-payload.xml");
const schemaPath =
  process.env.BENCH_SCHEMA ??
  resolve(fixtureDir, "../../../scenarios/xsd-validation-bridge/shared/schema.xsd");
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
//
// Engine init placement (rc-audm.1): identical to the node-native
// fixture — ONE persistent worker, wasm module compiled ONCE (the
// startup self-test below stays the engine init slot), per tick an
// instantiation from the cached module and a re-run of xmllint
// against the SAME engine artifacts from the pinned dependency (the
// library's validateXML API would instead re-boot a worker thread per
// tick — ~42ms/tick while the validation itself is ~1ms). See the
// node-native fixture header for the full engine-worker extraction.
const nodeRequire = createRequire(import.meta.url);
const engineDir = nodeRequire("path").dirname(nodeRequire.resolve("xmllint-wasm"));
const engineWorkerSrc = `
const { parentPort, workerData } = require("node:worker_threads");
const fs = require("node:fs");
// Requiring the engine's worker entry also registers ITS message
// listener, but it ignores messages without its 'xmllint-wasm' tag —
// only this handler acts. Its emscripten Module factory is reused
// verbatim, so the engine build and the argument shape are the
// library's own (memoryPages defaults 256/512 -> 16/32 MiB).
const Factory = require(workerData.xmllintNodePath);
let cachedModule = null;
parentPort.on("message", async (data) => {
  try {
    cachedModule ??= await WebAssembly.compile(fs.readFileSync(workerData.wasmPath));
    const wasmMemory = new WebAssembly.Memory({
      initial: data.initialMemory,
      maximum: data.maxMemory,
    });
    const result = await new Promise((resolveDone) => {
      let stdout = "";
      let stderr = "";
      Factory({
        inputFiles: data.inputFiles,
        arguments: data.args,
        wasmMemory,
        instantiateWasm(imports, success) {
          // instantiate(module, imports) resolves to the Instance
          // itself — the glue callback expects (instance, module).
          WebAssembly.instantiate(cachedModule, imports).then((inst) => {
            success(inst, cachedModule);
          });
        },
        print(text) { stdout += text + "\\n"; },
        printErr(text) { stderr += text + "\\n"; },
        onExit: (exitCode) => resolveDone({ exitCode, stdout, stderr }),
        onAbort: (reason) =>
          resolveDone({ exitCode: -1, stdout: "", stderr: "WASM Abort: " + reason }),
      });
    });
    // Same exit-code mapping as the library's validationSucceeded().
    const valid =
      result.exitCode === 0
        ? true
        : result.exitCode === 3 || result.exitCode === 4
          ? false
          : null;
    parentPort.postMessage(
      valid === null
        ? { error: result.stderr }
        : { valid, normalized: result.stdout, rawOutput: result.stderr },
    );
  } catch (err) {
    parentPort.postMessage({ error: String((err && err.stack) || err) });
  }
});
`;

const engineWorker = new Worker(engineWorkerSrc, {
  eval: true,
  workerData: {
    xmllintNodePath: `${engineDir}/xmllint-node.js`,
    wasmPath: `${engineDir}/xmllint.wasm`,
  },
});

// One validation in flight at a time (the timer route never overlaps
// route executions), so a single pending slot pairs each request with
// its response; a worker crash rejects the in-flight validate, which
// aborts the process non-zero like a failing validator step.
let pending = null;
engineWorker.on("message", (msg) => {
  const p = pending;
  pending = null;
  p?.resolve(msg);
});
engineWorker.on("error", (err) => {
  const p = pending;
  pending = null;
  p?.reject(err);
});

function validateBenchPayload() {
  return new Promise((resolve, reject) => {
    pending = { resolve, reject };
    engineWorker.postMessage({
      inputFiles: [
        { fileName: "bench-payload.xml", contents: payload },
        { fileName: "schema.xsd", contents: schema },
      ],
      // The argument shape the library builds per call
      // (preprocessOptions): --schema <file> --noout <xml>.
      args: ["--schema", "schema.xsd", "--noout", "bench-payload.xml"],
      initialMemory: 256,
      maxMemory: 512,
    });
  });
}

function validationDetail(result) {
  return (
    (result.rawOutput && result.rawOutput.trim()) ||
    JSON.stringify(result.errors)
  );
}

// Startup self-test = the wasm init slot (JVM counterpart: Xerces
// schema compile at route start; node: wasm module compile in the
// persistent worker). Invalid payload -> non-zero exit BEFORE the
// marker.
try {
  const selfTest = await validateBenchPayload();
  if (!selfTest.valid) {
    console.error(`error: xsd validation failed: ${validationDetail(selfTest)}`);
    process.exit(1);
  }
} catch (err) {
  console.error(`error: xsd validation threw during self-test: ${err}`);
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
