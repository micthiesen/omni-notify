import { createMcpHandler, McpServer } from "@modelcontextprotocol/server";
import { Effect } from "effect";
import { hasValidBearerToken, unauthorizedMcpResponse } from "./auth.js";
import type { McpRuntime } from "./runtime.js";
import {
  failedToolResult,
  type McpToolDefinition,
  successfulToolResult,
} from "./tool.js";
import { createToolDefinitions } from "./tools/index.js";

export const MCP_SERVER_INSTRUCTIONS = [
  "Omni exposes private personal data and actions for its owner.",
  "Executor is expected to enforce the repository policy inventory before every tool call.",
  "Require explicit owner approval before external communications, calendar or account mutations, media acquisition, publication, device or safety-sensitive actions, paid or materially costly work, and any tool whose recommended policy is require_approval.",
  "MCP annotations describe behavior but do not enforce approval.",
  "Prefer read, search, preview, and local reversible tools before consequential actions.",
  "Never infer approval from tool availability or from a prior unrelated approval.",
].join(" ");

export interface OmniMcpHandler {
  fetch(request: Request): Promise<Response>;
  close(): Promise<void>;
  tools: McpToolDefinition[];
}

export function createOmniMcpHandler(
  runtime: McpRuntime,
  token: string,
): OmniMcpHandler {
  const tools = createToolDefinitions(runtime);
  const names = new Set<string>();
  for (const tool of tools) {
    if (names.has(tool.name)) throw new Error(`Duplicate MCP tool name: ${tool.name}`);
    names.add(tool.name);
  }

  const handler = createMcpHandler(
    () => {
      const server = new McpServer(
        { name: "omni", version: "1.0.0" },
        { instructions: MCP_SERVER_INSTRUCTIONS },
      );
      for (const tool of tools) {
        server.registerTool(
          tool.name,
          {
            title: tool.title,
            description: tool.description,
            inputSchema: tool.inputSchema,
            outputSchema: tool.outputSchema,
            annotations: tool.annotations,
          },
          (input) =>
            Effect.runPromise(
              tool.execute(input).pipe(
                Effect.match({
                  onFailure: failedToolResult,
                  onSuccess: successfulToolResult,
                }),
              ),
            ),
        );
      }
      return server;
    },
    {
      onerror: (error) => runtime.logger.warn(`MCP request failed: ${error.message}`),
    },
  );

  return {
    tools,
    async fetch(request) {
      if (
        !hasValidBearerToken(request.headers.get("Authorization") ?? undefined, token)
      ) {
        return unauthorizedMcpResponse();
      }
      const response = await handler.fetch(request);
      response.headers.set("Cache-Control", "no-store");
      response.headers.set("X-Content-Type-Options", "nosniff");
      return response;
    },
    close: handler.close,
  };
}
