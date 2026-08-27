import type { McpToolDefinition } from "./tool.js";

export const MCP_POLICY_SCHEMA_VERSION = 1;

export function buildPolicyInventory(tools: McpToolDefinition[]) {
  return {
    schemaVersion: MCP_POLICY_SCHEMA_VERSION,
    generatedFrom: "src/mcp tool definitions",
    tools: tools
      .map((tool) => ({
        name: tool.name,
        title: tool.title,
        description: tool.description,
        annotations: tool.annotations,
        sideEffects: tool.policy.sideEffects,
        cost: tool.policy.cost,
        recommendedExecutorPolicy: tool.policy.recommendedPolicy,
      }))
      .sort((a, b) => a.name.localeCompare(b.name)),
  };
}

export function serializePolicyInventory(tools: McpToolDefinition[]): string {
  return `${JSON.stringify(buildPolicyInventory(tools), null, 2)}\n`;
}
