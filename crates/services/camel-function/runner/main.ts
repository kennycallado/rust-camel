import { Camel, CamelHandler, PatchWire } from "./camel.ts";

const registrations = new Map<string, { handler: CamelHandler; source: string }>();

function jsonResponse(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function handleHealth(): Response {
  return jsonResponse({
    status: "ready",
    registered: Array.from(registrations.keys()),
  });
}

async function handleRegister(req: Request): Promise<Response> {
  let body: { function_id?: string; runtime?: string; source?: string; timeout_ms?: number };
  try {
    body = await req.json();
  } catch {
    return jsonResponse({ error: "invalid JSON", kind: "compile_error" }, 400);
  }

  const { function_id, runtime, source, timeout_ms } = body;
  if (!function_id || !runtime || !source || timeout_ms === undefined) {
    return jsonResponse({ error: "missing required fields", kind: "compile_error" }, 400);
  }

  if (runtime !== "deno" && runtime !== "typescript" && runtime !== "javascript") {
    return jsonResponse({ error: `unsupported runtime: ${runtime}`, kind: "invalid_runtime" }, 400);
  }

  const existing = registrations.get(function_id);
  if (existing) {
    if (existing.source === source) {
      return new Response(null, { status: 200 });
    }
    return jsonResponse(
      { error: `function ${function_id} already registered with different source`, kind: "duplicate" },
      400,
    );
  }

  let handler: CamelHandler;
  try {
    const dataUrl = `data:application/typescript,${encodeURIComponent(source)}`;
    const mod = await import(dataUrl);
    if (typeof mod.default !== "function") {
      return jsonResponse({ error: "handler must export a default function", kind: "compile_error" }, 400);
    }
    handler = mod.default;
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    return jsonResponse({ error: message, kind: "compile_error" }, 400);
  }

  registrations.set(function_id, { handler, source });
  return new Response(null, { status: 204 });
}

async function handleInvoke(req: Request): Promise<Response> {
  let body: {
    function_id?: string;
    correlation_id?: string;
    body?: { kind: string; value?: unknown };
    headers?: Record<string, unknown>;
    properties?: Record<string, unknown>;
    timeout_ms?: number;
  };
  try {
    body = await req.json();
  } catch {
    return jsonResponse({
      ok: false,
      error: { kind: "user_error", message: "invalid request body" },
    });
  }

  const { function_id, correlation_id: _correlation_id, body: wireBody, headers, properties, timeout_ms } = body;
  const entry = registrations.get(function_id ?? "");
  if (!entry) {
    return jsonResponse({
      ok: false,
      error: { kind: "not_registered", message: `function ${function_id} not registered` },
    });
  }

  const decodedBody = wireBody
    ? wireBody.kind === "empty" || wireBody.kind === "bytes"
      ? null
      : wireBody.kind === "text"
        ? (wireBody.value as string | null)
        : wireBody.value ?? null
    : null;

  const camel = new Camel(
    decodedBody,
    headers ?? {},
    properties ?? {},
  );

  const timeout = Math.max(1, (timeout_ms ?? 5000) - 100);
  try {
    await Promise.race([
      Promise.resolve().then(() => entry.handler(camel)),
      new Promise<void>((_, reject) =>
        setTimeout(() => reject(new Error("timeout")), timeout)
      ),
    ]);
    const patch: PatchWire = camel.collectPatch();
    return jsonResponse({
      ok: true,
      patch: {
        body: patch.body ?? null,
        headers_set: patch.headers_set ?? [],
        headers_removed: patch.headers_removed ?? [],
        properties_set: patch.properties_set ?? [],
      },
    });
  } catch (e) {
    if (e instanceof Error && e.message === "timeout") {
      return jsonResponse({
        ok: false,
        error: { kind: "timeout", message: `function ${function_id} timed out after ${timeout}ms` },
      });
    }
    const message = e instanceof Error ? e.message : String(e);
    const stack = e instanceof Error ? e.stack ?? null : null;
    return jsonResponse({
      ok: false,
      error: { kind: "user_error", message, stack },
    });
  }
}

const port = Number(Deno.env.get("PORT") ?? "8000");
Deno.serve({ hostname: "0.0.0.0", port }, async (req: Request) => {
  const url = new URL(req.url);
  if (req.method === "GET" && url.pathname === "/health") return handleHealth();
  if (req.method === "POST" && url.pathname === "/register") return await handleRegister(req);
  if (req.method === "POST" && url.pathname === "/invoke") return await handleInvoke(req);
  if (req.method === "POST" && url.pathname === "/shutdown") {
    setTimeout(() => Deno.exit(0), 100);
    return new Response(null, { status: 204 });
  }
  return new Response("not found", { status: 404 });
});
