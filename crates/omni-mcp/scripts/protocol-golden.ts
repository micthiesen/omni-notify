/**
 * Offline MCP protocol capture for the Rust port (WP12).
 *
 * Builds the TS MCP handler (`src/mcp/server.ts`) with an in-memory docstore, a
 * fake task registry and an MCP Events service whose webhook and Executor
 * authorization are fakes, drives it with the official client in both protocol
 * eras plus raw edge-case requests, and records every raw HTTP exchange
 * (request headers and body, response status, content type and JSON-RPC
 * messages). Never reaches the network.
 *
 * Usage: npx tsx crates/omni-mcp/scripts/protocol-golden.ts <out.json>
 */
import { writeFile } from "node:fs/promises";
import { Client, StreamableHTTPClientTransport } from "@modelcontextprotocol/client";
import { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { z } from "zod";
import { createMitoolsTestRuntime } from "../../../src/test/mitools.js";
import type { McpRuntime } from "../../../src/mcp/runtime.js";
import { createOmniMcpHandler } from "../../../src/mcp/server.js";
import { McpEventService } from "../../../src/mcp/events/service.js";
import type { TaskRegistry } from "../../../src/task-runs/registry.js";

const TOKEN = "golden-capture-token-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const OWNER = `executor:${"a".repeat(64)}`;
const DELEGATED = "Bearer delegated-fixture";
const SECRET = `whsec_${Buffer.alloc(32, 9).toString("base64")}`;
const CALLBACK = "https://chatgpt.example.com/events/callback";

const mitools = createMitoolsTestRuntime();

const events = Effect.runSync(
  McpEventService.make(
    TOKEN,
    (owner, bearer) =>
      Effect.succeed(owner === OWNER && bearer === DELEGATED ? Date.now() + 3_600_000 : null),
    {
      verify: () => Effect.void,
      deliver: () => Effect.succeed({ status: 204 }),
    },
  ),
);

const runtime: McpRuntime = {
  logger: Logger.named("McpProtocolGolden"),
  effectRunner: mitools.runner,
  registry: {
    list: () => Effect.succeed([]),
    runNow: () => Effect.succeed({ runId: "golden-run" }),
  } as unknown as TaskRegistry,
  streamers: [],
  emailControls: {},
  events,
};

type Exchange = {
  label: string;
  request: { method: string; headers: Record<string, string>; body: unknown };
  response: { status: number; contentType: string | null; headers: Record<string, string>; messages: unknown[]; text: string };
};

const exchanges: Exchange[] = [];
let label = "";
const handler = createOmniMcpHandler(runtime, TOKEN);

function parseBody(text: string): unknown[] {
  const trimmed = text.trim();
  if (!trimmed) return [];
  if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
    const parsed = JSON.parse(trimmed);
    return Array.isArray(parsed) ? parsed : [parsed];
  }
  return trimmed
    .split(/\r?\n/)
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice(5).trim())
    .filter(Boolean)
    .map((data) => JSON.parse(data));
}

const KEPT_RESPONSE_HEADERS = ["cache-control", "x-content-type-options", "www-authenticate", "allow"];

async function record(request: Request): Promise<Response> {
  const text = request.method === "POST" ? await request.clone().text() : "";
  let body: unknown = text;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = text;
  }
  const response = await handler.fetch(request);
  const responseText = await response.clone().text();
  let messages: unknown[] = [];
  try {
    messages = parseBody(responseText);
  } catch {
    messages = [];
  }
  exchanges.push({
    label,
    request: {
      method: request.method,
      headers: Object.fromEntries(
        [...request.headers.entries()].filter(([key]) => key.toLowerCase() !== "authorization"),
      ),
      body,
    },
    response: {
      status: response.status,
      contentType: response.headers.get("content-type"),
      headers: Object.fromEntries(
        [...response.headers.entries()].filter(([key]) => KEPT_RESPONSE_HEADERS.includes(key)),
      ),
      messages,
      text: messages.length === 0 ? responseText : "",
    },
  });
  return response;
}

function client(mode: "legacy" | "auto", extraHeaders: Record<string, string> = {}) {
  const transport = new StreamableHTTPClientTransport(new URL("http://mcp.golden/mcp"), {
    requestInit: { headers: { Authorization: `Bearer ${TOKEN}`, ...extraHeaders } },
    fetch: (input: RequestInfo | URL, init?: RequestInit) => record(new Request(input, init)),
  });
  return {
    transport,
    client: new Client({ name: "omni-golden", version: "1.0.0" }, { versionNegotiation: { mode } }),
  };
}

const subscribeParams = (url = CALLBACK) => ({
  name: "email.received",
  arguments: { folder: "inbox" },
  delivery: { mode: "webhook", url, secret: SECRET },
});

async function attempt(name: string, run: () => Promise<unknown>) {
  label = name;
  try {
    await run();
  } catch {
    // The raw exchange is what the golden keeps.
  }
}

async function session(mode: "legacy" | "auto", prefix: string, headers: Record<string, string> = {}) {
  const { client: c, transport } = client(mode, headers);
  label = `${prefix}connect`;
  await c.connect(transport);
  await attempt(`${prefix}tools/list`, () => c.listTools());
  await attempt(`${prefix}tools/call ok`, () => c.callTool({ name: "tasks_list", arguments: {} }));
  await attempt(`${prefix}tools/call invalid input`, () =>
    c.callTool({ name: "tasks_list", arguments: { cursor: -1, unexpected: true } }),
  );
  await attempt(`${prefix}tools/call mutation`, () =>
    c.callTool({ name: "task_run", arguments: { taskName: "MockTask" } }),
  );
  await attempt(`${prefix}tools/call unknown tool`, () =>
    c.callTool({ name: "no_such_tool", arguments: {} }),
  );
  await attempt(`${prefix}ping`, () => c.ping());
  await attempt(`${prefix}events/list`, () =>
    c.request({ method: "events/list", params: {} }, z.unknown()),
  );
  await attempt(`${prefix}events/subscribe`, () =>
    c.request({ method: "events/subscribe", params: subscribeParams() }, z.unknown()),
  );
  await attempt(`${prefix}events/subscribe invalid callback`, () =>
    c.request(
      {
        method: "events/subscribe",
        params: subscribeParams("http://chatgpt.example.com/events/callback"),
      },
      z.unknown(),
    ),
  );
  await attempt(`${prefix}events/subscribe invalid params`, () =>
    c.request({ method: "events/subscribe", params: { name: "email.received" } }, z.unknown()),
  );
  await attempt(`${prefix}events/subscribe invalid event`, () =>
    c.request(
      { method: "events/subscribe", params: { ...subscribeParams(), name: "nope" } },
      z.unknown(),
    ),
  );
  await attempt(`${prefix}events/unsubscribe`, () =>
    c.request(
      {
        method: "events/unsubscribe",
        params: {
          name: "email.received",
          arguments: { folder: "inbox" },
          delivery: { mode: "webhook", url: CALLBACK },
        },
      },
      z.unknown(),
    ),
  );
  await attempt(`${prefix}unknown method`, () =>
    c.request({ method: "resources/list", params: {} }, z.unknown()),
  );
  await c.close();
}

async function raw(name: string, init: RequestInit & { headers?: Record<string, string> }) {
  label = name;
  await record(
    new Request("http://mcp.golden/mcp", {
      ...init,
      headers: { Authorization: `Bearer ${TOKEN}`, ...(init.headers ?? {}) },
    }),
  );
}

await session("legacy", "legacy ");
await session("auto", "modern ");
await session("auto", "delegated ", {
  "x-omni-events-owner": OWNER,
  "x-omni-events-authorization": DELEGATED,
});
await session("legacy", "bad principal ", {
  "x-omni-events-owner": "executor:short",
  "x-omni-events-authorization": DELEGATED,
});

const json = { "content-type": "application/json", accept: "application/json, text/event-stream" };
await raw("raw GET", { method: "GET", headers: { accept: "text/event-stream" } });
await raw("raw DELETE", { method: "DELETE" });
await raw("raw text/plain", {
  method: "POST",
  headers: { "content-type": "text/plain", accept: json.accept },
  body: "{}",
});
await raw("raw unparseable", { method: "POST", headers: json, body: "{" });
await raw("raw empty body", { method: "POST", headers: json, body: "" });
await raw("raw notification", {
  method: "POST",
  headers: json,
  body: JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }),
});
await raw("raw tools/list without initialize", {
  method: "POST",
  headers: json,
  body: JSON.stringify({ jsonrpc: "2.0", id: 7, method: "tools/list", params: {} }),
});
await raw("raw events/list legacy without initialize", {
  method: "POST",
  headers: json,
  body: JSON.stringify({ jsonrpc: "2.0", id: 8, method: "events/list", params: {} }),
});
await raw("raw adapter events/list", {
  method: "POST",
  headers: {
    ...json,
    "mcp-protocol-version": "2026-07-28",
    "mcp-method": "events/list",
    "x-omni-events-owner": OWNER,
    "x-omni-events-authorization": DELEGATED,
  },
  body: JSON.stringify({
    jsonrpc: "2.0",
    id: 9,
    method: "events/list",
    params: {
      _meta: {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { name: "chatgpt", version: "1" },
        "io.modelcontextprotocol/clientCapabilities": {},
      },
    },
  }),
});
await raw("raw adapter events/subscribe", {
  method: "POST",
  headers: {
    ...json,
    "mcp-protocol-version": "2026-07-28",
    "mcp-method": "events/subscribe",
    "x-omni-events-owner": OWNER,
    "x-omni-events-authorization": DELEGATED,
  },
  body: JSON.stringify({
    jsonrpc: "2.0",
    id: 10,
    method: "events/subscribe",
    params: {
      ...subscribeParams("https://chatgpt.example.com/events/adapter"),
      _meta: {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { name: "chatgpt", version: "1" },
        "io.modelcontextprotocol/clientCapabilities": {},
      },
    },
  }),
});
await raw("raw batch", {
  method: "POST",
  headers: json,
  body: JSON.stringify([
    { jsonrpc: "2.0", id: 11, method: "ping" },
    { jsonrpc: "2.0", id: 12, method: "tools/list" },
  ]),
});
label = "raw unauthorized";
exchanges.push({
  label,
  request: { method: "POST", headers: json, body: { jsonrpc: "2.0", id: 1, method: "initialize" } },
  response: await (async () => {
    const response = await handler.fetch(
      new Request("http://mcp.golden/mcp", {
        method: "POST",
        headers: { ...json, Authorization: "Bearer wrong" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize" }),
      }),
    );
    const text = await response.text();
    return {
      status: response.status,
      contentType: response.headers.get("content-type"),
      headers: Object.fromEntries(
        [...response.headers.entries()].filter(([key]) => KEPT_RESPONSE_HEADERS.includes(key)),
      ),
      messages: [JSON.parse(text)],
      text: "",
    };
  })(),
});

await handler.close();
await mitools.dispose();
const out = process.argv[2];
if (!out) throw new Error("usage: protocol-golden.ts <out.json>");
await writeFile(out, `${JSON.stringify({ generatedFrom: "src/mcp/server.ts (offline, fake services)", exchanges }, null, 2)}\n`);
