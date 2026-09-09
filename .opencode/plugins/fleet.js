// .opencode/plugins/fleet.js — in-process fleet listener (replaces .fleet/listener.mjs)
//
// Hooks the server event bus directly: no SSE client, no reconnect races,
// no systemd transient unit. Loaded automatically by every opencode instance
// for this project; only the `serve` instance acts (clients would double-fire).
//
// Ported 1:1 from .fleet/listener.mjs (2026-09-09): same buzzer format,
// same wake protocol text, same 60s per-session debounce, same events.log.
// Improvement: fleet.json / conductor.json are re-read per event (live roster
// updates without server restart — the old listener read them once at boot).
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

function stateDir() {
  for (const d of [`${PLUGIN_DIR}../fleet/`, `${REPO}.fleet/`]) {
    if (existsSync(d + "fleet.json")) return d;
  }
  return `${REPO}.fleet/`; // default; logged on first event if missing
}

const log = (o) =>
  appendFileSync(`${stateDir()}events.log`, JSON.stringify({ t: new Date().toISOString(), src: "plugin", ...o }) + "\n");

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
    const res = await fetch(`${BASE}/session/${conductor}/message`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ parts: [{ type: "text", text }] }),
    });
    log({ ev: "wake-posted", to: conductor.slice(0, 16), http: res.status });
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
        let fleet = [];
        try {
          fleet = JSON.parse(readFileSync(`${dir}fleet.json`, "utf8"));
        } catch {}
        if (!fleet.includes(sid)) return;
        appendFileSync(`${dir}buzzer`, `${new Date().toISOString()} ${type} ${sid}\n`);
        log({ ev: type, sid: String(sid).slice(0, 24), fleet: true });
        await wakeConductor(type, sid, dir);
      } catch (e) {
        log({ ev: "plugin-error", err: String(e).slice(0, 200) });
      }
    },
  };
};
