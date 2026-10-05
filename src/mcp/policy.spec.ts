import { readFile } from "node:fs/promises";
import { Logger } from "@micthiesen/mitools/logging";
import { afterAll, describe, expect, it } from "vitest";
import { createMitoolsTestRuntime } from "../test/mitools.js";
import type { TaskRegistry } from "../task-runs/registry.js";
import { z } from "zod";
import { serializePolicyInventory } from "./policy.js";
import { MCP_SERVER_INSTRUCTIONS } from "./server.js";
import type { McpRuntime } from "./runtime.js";
import { createToolDefinitions } from "./tools/index.js";

const testRuntime = createMitoolsTestRuntime();
afterAll(() => testRuntime.dispose());

function policyTools() {
  const runtime: McpRuntime = {
    logger: Logger.named("McpPolicySpec"),
    effectRunner: testRuntime.runner,
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

  it("never names the machine behind any tool", () => {
    const machineWords = /\b(mac|macs|macbook|maxbook|macos|laptop|eventkit)\b/i;
    const surfaces = policyTools().map((tool) => ({
      name: tool.name,
      text: JSON.stringify([
        tool.title,
        tool.description,
        tool.policy,
        z.toJSONSchema(tool.inputSchema, { io: "input", unrepresentable: "any" }),
        z.toJSONSchema(tool.outputSchema, { unrepresentable: "any" }),
      ]),
    }));
    surfaces.push({ name: "server instructions", text: MCP_SERVER_INSTRUCTIONS });
    expect(surfaces.filter(({ text }) => machineWords.test(text))).toEqual([]);
  });
});
