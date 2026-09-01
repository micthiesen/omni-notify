import type { Hono } from "hono";
import { Effect } from "effect";
import { effectHandler } from "../effect/http.js";
import { fromPromise } from "../effect/interop.js";
import type { McpRuntime } from "./runtime.js";
import { createOmniMcpHandler, type OmniMcpHandler } from "./server.js";

/** Register the production MCP route and return its lifecycle handle. */
export function registerOmniMcpRoute(
  app: Hono,
  runtime: McpRuntime,
  token?: string,
): OmniMcpHandler | undefined {
  const mcp = token ? createOmniMcpHandler(runtime, token) : undefined;

  app.all(
    "/mcp",
    effectHandler((context) =>
      mcp
        ? fromPromise("serve MCP request", () => mcp.fetch(context.req.raw))
        : Effect.succeed(context.json({ error: "MCP is not configured" }, 503)),
    ),
  );

  return mcp;
}
