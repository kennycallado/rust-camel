// .opencode/plugins/fleet.js — in-process fleet listener (replaces .fleet/listener.mjs)
//
// Hooks the server event bus directly: no SSE client, no reconnect races,
// no systemd transient unit. Loaded automatically by every opencode instance
// for this project; only the `serve` instance acts (clients would double-fire).
//
// Hardened 2026-09-12 after the silent-wake outage (root cause: fleet.json
// entries drifted from plain sid strings to rich mission objects, and
// `includes(sid)` never matches objects — every mission registered after the
// drift went silent). Changes vs previous version:
//   1. Roster check accepts BOTH entry shapes: "ses_…" strings and
//      { "session": "ses_…", … } mission objects (live-read per event).
//   2. Wake POST has a 10 s timeout (the old listener logged hung POST
//      TimeoutErrors; never let a wake hang unbounded).
//   3. Every WAKE event for an unidentified session is logged once per sid
//      (ev:"wake-event-miss") — during the outage, misses were invisible,
//      which made the plugin look dead when it was merely filtering.
//
// State dir resolution (import.meta.url based — server cwd independent):
//   1. <repo>/.opencode/fleet/   (target home, post-migration)
//   2. <repo>/.fleet/            (current)
// where <repo> = two levels up from this file's directory.
//
// Wake POST target: env OPENCODE_SERVER || http://localhost:8080
// (the server always listens on its own port; loopback fetch is safe).
import { appendFileSync, existsSync, mkdirSync, readFileSync } from "node:fs";

const BASE = process.env.OPENCODE_SERVER || "http://localhost:8080";
const PLUGIN_DIR = new URL(".", import.meta.url).pathname; // <repo>/.opencode/plugins/
const REPO = new URL("../..", import.meta.url).pathname; // <repo>/

// only the serve instance acts; run/tui clients no-op (they load this too)
// argv convention: [bin, 'serve', ...] — if probe shows silent no-op, log argv here
const AM_SERVE = process.argv[2] === "serve";

const WAKE_EVENTS = new Set([
  "session.idle",
  "session.completed",
  "session.deleted",
  "session.error",
]);
const WAKE_DEBOUNCE_MS = 60_000;
const lastWake = new Map();
const loggedMiss = new Set(); // sids already logged as wake-event-miss (once each)

function stateDir() {
  for (const d of [`${PLUGIN_DIR}../fleet/`, `${REPO}.fleet/`]) {
    if (existsSync(d + "fleet.json")) return d;
  }
  return `${REPO}.fleet/`; // default; logged on first event if missing
}

const log = (o) =>
  appendFileSync(`${stateDir()}events.log`, JSON.stringify({ t: new Date().toISOString(), src: "plugin", ...o }) + "\n");

// fleet.json entries may be plain sid strings ("ses_…") or mission objects
// ({ "session": "ses_…", … }). Normalize both to a Set of ids.
function rosterIds(dir) {
  try {
    const entries = JSON.parse(readFileSync(`${dir}fleet.json`, "utf8"));
    const ids = new Set();
    for (const e of entries) {
      const sid = typeof e === "string" ? e : e?.session;
      if (typeof sid === "string" && sid) ids.add(sid);
    }
    return ids;
  } catch {
    return new Set();
  }
}

async function wakeConductor(type, sid, dir) {
  let conductor = "";
  try {
    conductor = JSON.parse(readFileSync(`${dir}conductor.json`, "utf8")).conductor;
  } catch {}
  if (!conductor) return log({ ev: "wake-skipped", reason: "no conductor id" });
  const now = Date.now();
  if (now - (lastWake.get(sid) || 0) < WAKE_DEBOUNCE_MS) {
    return log({ ev: "wake-debounced", sid: String(sid).slice(0, 24) });
  }
  lastWake.set(sid, now);
  const text =
    `[SYSTEM-WAKE via fleet plugin — NOT the human. ` +
    `Event ${type} on fleet session ${sid}. ` +
    `Protocol: (1) read state-dir buzzer (clear processed lines), ` +
    `(2) read new files in inbox/, ` +
    `(3) act per conductor duties: verify PARKED reports, order landings ` +
    `(systemd-run opencode run --attach ${BASE} --session <sid>), ` +
    `or inject continuation if a mission stalled mid-turn. ` +
    `(4) If nothing actionable, reply briefly and stop — do not spin.]`;
  try {
    // 2026-09-20: server enforces Basic auth — resolve the password from
    // the server process env first, then the canonical server-auth file.
    let authHeader = "";
    try {
      const pass =
        process.env.OPENCODE_SERVER_PASSWORD ||
        (await import("node:fs")).readFileSync(
          `${REPO}.opencode/fleet/server-auth`,
          "utf8",
        ).trim();
      authHeader =
        "Basic " + Buffer.from("opencode:" + pass).toString("base64");
    } catch {}
    const res = await fetch(`${BASE}/session/${conductor}/message`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(authHeader ? { Authorization: authHeader } : {}),
      },
      body: JSON.stringify({ parts: [{ type: "text", text }] }),
      signal: AbortSignal.timeout(10_000),
    });
    log({ ev: "wake-posted", to: conductor.slice(0, 16), http: res.status });
    if (res.status !== 200) {
      // 2026-09-13 incident: stale conductor.json id → silent 404, parked
      // mission invisible for hours. A failed delivery must scream at the
      // desktop so the human notices even when the conductor is deaf.
      log({ ev: "wake-lost", to: conductor.slice(0, 16), http: res.status });
      const note =
        `FLEET WAKE LOST (http ${res.status}) — fix .opencode/fleet/conductor.json`;
      try {
        const { execFileSync } = await import("node:child_process");
        execFileSync("notify-send", ["-u", "critical", "camel-fleet", note], {
          stdio: "ignore",
        });
      } catch {}
    }
  } catch (e) {
    log({ ev: "wake-error", err: String(e).slice(0, 150) });
  }
}

export const FleetPlugin = async () => {
  if (!AM_SERVE) return {}; // clients no-op
  const dir = stateDir();
  for (const d of [dir, dir + "inbox/"]) if (!existsSync(d)) mkdirSync(d, { recursive: true });
  log({ ev: "plugin-loaded", base: BASE, dir });
  return {
    event: async ({ event }) => {
      try {
        const type = event?.type || "";
        if (!WAKE_EVENTS.has(type)) return;
        const sid = event?.properties?.sessionID || event?.properties?.session_id || "";
        if (!sid) return;
        // live roster read (no restart needed to update fleet.json)
        const fleet = rosterIds(dir);
        if (!fleet.has(sid)) {
          if (!loggedMiss.has(sid)) {
            loggedMiss.add(sid);
            log({ ev: "wake-event-miss", type, sid: String(sid).slice(0, 24) });
          }
          return;
        }
        appendFileSync(`${dir}buzzer`, `${new Date().toISOString()} ${type} ${sid}\n`);
        log({ ev: type, sid: String(sid).slice(0, 24), fleet: true });
        await wakeConductor(type, sid, dir);
      } catch (e) {
        log({ ev: "plugin-error", err: String(e).slice(0, 200) });
      }
    },
  };
};
