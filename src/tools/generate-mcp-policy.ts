import { writeFile } from "node:fs/promises";
import { Logger } from "@micthiesen/mitools/logging";
import { serializePolicyInventory } from "../mcp/policy.js";
import type { McpRuntime } from "../mcp/runtime.js";
import { createToolDefinitions } from "../mcp/tools/index.js";
import type { TaskRegistry } from "../task-runs/registry.js";

const logger = new Logger("McpPolicyGenerator");
const inertRegistry = {
  list: () => [],
  runNow: () => {
    throw new Error("Policy generation cannot run tasks");
  },
} as unknown as TaskRegistry;

const runtime: McpRuntime = {
  logger,
  registry: inertRegistry,
  streamers: [],
  emailControls: {},
};

await writeFile(
  new URL("../../docs/mcp-policy.json", import.meta.url),
  serializePolicyInventory(createToolDefinitions(runtime)),
  "utf8",
);
