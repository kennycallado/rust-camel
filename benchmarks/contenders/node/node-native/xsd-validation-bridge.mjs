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
//   start; node forces the wasm module compile here (persistent
//   worker, see below) so measured ticks pay only the per-call
//   engine cost. Placement rationale: see README.

import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Worker } from "node:worker_threads";

const fixtureDir = dirname(fileURLToPath(import.meta.url));

const payloadPath =
  process.env.BENCH_PAYLOAD ??
  resolve(fixtureDir, "../../../scenarios/xsd-validation-bridge/shared/bench-payload.xml");
const schemaPath =
  process.env.BENCH_SCHEMA ??
  resolve(fixtureDir, "../../../scenarios/xsd-validation-bridge/shared/schema.xsd");
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
//
// Engine init placement (rc-audm.1): the library's validateXML API
// spawns a FRESH worker thread per call — worker boot, wasm
// fetch/compile/instantiate, schema parse and terminate every tick
// (~42ms/tick measured, while the validation itself is ~1ms). This
// fixture therefore owns ONE persistent worker instead: the wasm
// module is WebAssembly.compile-ed ONCE (the startup self-test below
// stays the engine init slot — the node counterpart of the JVM's
// once-per-process Xerces schema compile) and every tick
// instantiates from the cached module and re-runs xmllint against
// the SAME engine artifacts (xmllint.wasm / xmllint-node.js from the
// pinned dependency — engine behavior unchanged, no new dependency).
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
// marker — exactly once.
setTimeout(fireTick, PERIOD_MS);
console.log(`BENCH_ROUTE_READY ${Date.now()}`);

// repeatCount exhausted -> idle until killed, like ctrl_c().await.
setInterval(() => {}, 1 << 30);
