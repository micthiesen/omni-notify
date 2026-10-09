/**
 * Offline MCP golden capture: builds the TS MCP handler with inert services (as
 * `src/tools/generate-mcp-policy.ts` does), connects an in-process client in both
 * protocol eras and records the raw JSON-RPC results of the handshake and
 * `tools/list`. Never reaches the network or production.
 *
 * Writes `<dir>/tools-list.json` (the `tools/list` result, identical in both eras;
 * the script fails if they differ) and `<dir>/handshake.json` (legacy `initialize`,
 * modern `server/discover`, and each era's `tools/list` envelope without `tools`).
 *
 * Usage: tsx crates/xtask/node/mcp-golden.ts <dir>
 */
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { Client, StreamableHTTPClientTransport } from "@modelcontextprotocol/client";
import { Logger } from "@micthiesen/mitools/logging";
import type { EffectRunner } from "@micthiesen/mitools/boundary";
import type { AppServices } from "../../../src/effect/appRuntime.js";
import type { McpRuntime } from "../../../src/mcp/runtime.js";
import { createOmniMcpHandler } from "../../../src/mcp/server.js";
import type { TaskRegistry } from "../../../src/task-runs/registry.js";

const TOKEN = "golden-capture-token-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const inertRunner: EffectRunner<never> = {
  runPromise: () => Promise.reject(new Error("Golden capture cannot run effects")),
  runFork: () => {
    throw new Error("Golden capture cannot run effects");
  },
};
const runtime: McpRuntime = {
  logger: Logger.named("McpGolden"),
  effectRunner: inertRunner as unknown as EffectRunner<AppServices>,
  registry: {
    list: () => [],
    runNow: () => {
      throw new Error("Golden capture cannot run tasks");
    },
  } as unknown as TaskRegistry,
  streamers: [],
  emailControls: {},
};

type Exchange = { method: string; request: unknown; result: unknown; headers: Record<string, string> };

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

async function capture(mode: "legacy" | "auto") {
  const handler = createOmniMcpHandler(runtime, TOKEN);
  const exchanges: Exchange[] = [];
  const transport = new StreamableHTTPClientTransport(new URL("http://mcp.golden/mcp"), {
    requestInit: { headers: { Authorization: `Bearer ${TOKEN}` } },
    fetch: async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = new Request(input, init);
      const requestText = request.method === "POST" ? await request.clone().text() : "";
      const response = await handler.fetch(request);
      const responseText = await response.clone().text();
      const sent = requestText ? JSON.parse(requestText) : null;
      const received = parseBody(responseText) as Array<{ id?: unknown; result?: unknown }>;
      for (const message of Array.isArray(sent) ? sent : sent ? [sent] : []) {
        if (message?.id === undefined) continue;
        const reply = received.find((r) => r.id === message.id);
        exchanges.push({
          method: message.method,
          request: message.params ?? null,
          result: reply?.result ?? reply ?? null,
          headers: Object.fromEntries(
            [...request.headers.entries()].filter(([k]) => k.toLowerCase().startsWith("mcp-")),
          ),
        });
      }
      return response;
    },
  });
  const client = new Client(
    { name: "omni-golden", version: "1.0.0" },
    { versionNegotiation: { mode } },
  );
  await client.connect(transport);
  await client.listTools();
  await client.close();
  await handler.close();
  return exchanges;
}

const dir = process.argv[2];
if (!dir) throw new Error("usage: mcp-golden.ts <dir>");
const legacy = await capture("legacy");
const modern = await capture("auto");
const result = (exchanges: Exchange[], method: string) => {
  const found = exchanges.find((e) => e.method === method);
  if (!found) throw new Error(`no ${method} exchange captured`);
  return found.result as Record<string, unknown>;
};
const legacyList = result(legacy, "tools/list");
const modernList = result(modern, "tools/list");
if (JSON.stringify(legacyList.tools) !== JSON.stringify(modernList.tools)) {
  throw new Error("tools/list differs between protocol eras; extend the golden format");
}
const envelope = (list: Record<string, unknown>) =>
  Object.fromEntries(Object.entries(list).filter(([key]) => key !== "tools"));
await mkdir(dir, { recursive: true });
const json = (value: unknown) => `${JSON.stringify(value, null, 2)}\n`;
await writeFile(join(dir, "tools-list.json"), json({ tools: legacyList.tools }), "utf8");
await writeFile(
  join(dir, "handshake.json"),
  json({
    generatedFrom: "src/mcp/server.ts with inert services (offline)",
    legacy: {
      protocolVersion: legacy.find((e) => e.method === "tools/list")?.headers["mcp-protocol-version"],
      initialize: result(legacy, "initialize"),
      toolsListEnvelope: envelope(legacyList),
    },
    modern: {
      protocolVersion: modern.find((e) => e.method === "tools/list")?.headers["mcp-protocol-version"],
      discover: result(modern, "server/discover"),
      toolsListEnvelope: envelope(modernList),
    },
  }),
  "utf8",
);
