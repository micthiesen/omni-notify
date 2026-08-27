import type { Hono } from "hono";
import type { McpRuntime } from "./runtime.js";
import { createOmniMcpHandler, type OmniMcpHandler } from "./server.js";

/** Register the production MCP route and return its lifecycle handle. */
export function registerOmniMcpRoute(
  app: Hono,
  runtime: McpRuntime,
  token?: string,
): OmniMcpHandler | undefined {
  const mcp = token ? createOmniMcpHandler(runtime, token) : undefined;

  app.all("/mcp", async (c) => {
    if (!mcp) return c.json({ error: "MCP is not configured" }, 503);
    return await mcp.fetch(c.req.raw);
  });

  return mcp;
}
