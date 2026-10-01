import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { Effect } from "effect";
import type { AdapterOptions } from "./auth.js";

export interface ElicitationSupport {
  readonly form: boolean;
  readonly url: boolean;
}

export async function closeLegacyClient(client: Client): Promise<void> {
  let timer: NodeJS.Timeout | undefined;
  try {
    await Promise.race([
      client.close().catch(() => {}),
      new Promise<void>((resolve) => {
        timer = setTimeout(resolve, 2_000);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

export function openLegacyClient(
  options: AdapterOptions,
  authorization: string,
  query: URLSearchParams,
  interactive: ElicitationSupport = { form: false, url: false },
  configure?: (client: Client) => void,
) {
  return Effect.tryPromise({
    try: () =>
      connectLegacyClient(options, authorization, query, interactive, configure),
    catch: () => new Error("Executor MCP connection failed"),
  });
}

async function connectLegacyClient(
  options: AdapterOptions,
  authorization: string,
  query: URLSearchParams,
  interactive: ElicitationSupport,
  configure?: (client: Client) => void,
): Promise<Client> {
  const endpoint = new URL("/mcp", options.executorBaseUrl);
  // The connection's selected elicitation mode belongs to the tool call.
  const mode = query.get("elicitation_mode");
  if (mode === "native" || mode === "browser" || mode === "model") {
    endpoint.searchParams.set("elicitation_mode", mode);
  }
  const client = new Client(
    { name: "omni-executor-events-adapter", version: "0.1.0" },
    {
      capabilities:
        interactive.form || interactive.url
          ? {
              elicitation: {
                ...(interactive.form ? { form: {} } : {}),
                ...(interactive.url ? { url: {} } : {}),
              },
            }
          : {},
    },
  );
  const transport = new StreamableHTTPClientTransport(endpoint, {
    requestInit: { headers: { authorization } },
  });
  configure?.(client);
  try {
    await client.connect(transport, {
      signal: AbortSignal.timeout(10_000),
      timeout: 10_000,
    });
    return client;
  } catch (error) {
    await closeLegacyClient(client);
    throw error;
  }
}

export function callLegacy(
  client: Client,
  method: string,
  params: Record<string, unknown>,
) {
  return Effect.tryPromise({
    try: async () => {
      switch (method) {
        case "tools/list":
          return client.listTools(params as Parameters<Client["listTools"]>[0]);
        case "tools/call":
          return client.callTool(params as Parameters<Client["callTool"]>[0]);
        case "resources/list":
          return client.listResources(params as Parameters<Client["listResources"]>[0]);
        case "resources/read":
          return client.readResource(params as Parameters<Client["readResource"]>[0]);
        case "resources/templates/list":
          return client.listResourceTemplates(
            params as Parameters<Client["listResourceTemplates"]>[0],
          );
        case "prompts/list":
          return client.listPrompts(params as Parameters<Client["listPrompts"]>[0]);
        case "prompts/get":
          return client.getPrompt(params as Parameters<Client["getPrompt"]>[0]);
        default:
          return undefined;
      }
    },
    catch: () => new Error("Executor MCP request failed"),
  });
}
