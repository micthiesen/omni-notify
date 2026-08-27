import { readFile } from "node:fs/promises";
import { Logger } from "@micthiesen/mitools/logging";
import { describe, expect, it } from "vitest";
import type { TaskRegistry } from "../task-runs/registry.js";
import { serializePolicyInventory } from "./policy.js";
import type { McpRuntime } from "./runtime.js";
import { createToolDefinitions } from "./tools/index.js";

function policyTools() {
  const runtime: McpRuntime = {
    logger: new Logger("McpPolicySpec"),
    registry: { list: () => [] } as unknown as TaskRegistry,
    streamers: [],
    emailControls: {},
  };
  return createToolDefinitions(runtime);
}

describe("MCP policy inventory", () => {
  it("is complete and generated from the registered tool definitions", async () => {
    const tools = policyTools();
    const committed = await readFile(
      new URL("../../docs/mcp-policy.json", import.meta.url),
      "utf8",
    );
    expect(committed).toBe(serializePolicyInventory(tools));
    expect(new Set(tools.map(({ name }) => name)).size).toBe(tools.length);
    expect(tools.length).toBeGreaterThan(50);
  });

  it("records every annotation and a usable Executor policy for every tool", () => {
    for (const tool of policyTools()) {
      expect(tool.annotations).toEqual({
        readOnlyHint: expect.any(Boolean),
        destructiveHint: expect.any(Boolean),
        idempotentHint: expect.any(Boolean),
        openWorldHint: expect.any(Boolean),
      });
      expect(["allow", "require_approval", "block"]).toContain(
        tool.policy.recommendedPolicy,
      );
      expect(tool.policy.cost.length).toBeGreaterThan(0);
      expect(tool.description.length).toBeGreaterThan(20);
    }
  });
});
