import { writeFile } from "node:fs/promises";
import { Logger } from "@micthiesen/mitools/logging";
import { Data, Effect } from "effect";
import type { EffectRunner } from "@micthiesen/mitools/boundary";
import { serializePolicyInventory } from "../mcp/policy.js";
import type { McpRuntime } from "../mcp/runtime.js";
import { createToolDefinitions } from "../mcp/tools/index.js";
import type { TaskRegistry } from "../task-runs/registry.js";
import type { AppServices } from "../effect/appRuntime.js";

const logger = Logger.named("McpPolicyGenerator");
const inertRunner: EffectRunner<never> = {
  runPromise: () => Promise.reject(new Error("Policy generation cannot run effects")),
  runFork: () => {
    throw new Error("Policy generation cannot run effects");
  },
};
const inertRegistry = {
  list: () => [],
  runNow: () => {
    throw new Error("Policy generation cannot run tasks");
  },
} as unknown as TaskRegistry;

const runtime: McpRuntime = {
  logger,
  effectRunner: inertRunner as unknown as EffectRunner<AppServices>,
  registry: inertRegistry,
  streamers: [],
  emailControls: {},
};

class PolicyGenerationError extends Data.TaggedError("PolicyGenerationError")<{
  cause: unknown;
}> {}

await Effect.runPromise(
  Effect.tryPromise({
    try: () =>
      writeFile(
        new URL("../../docs/mcp-policy.json", import.meta.url),
        serializePolicyInventory(createToolDefinitions(runtime)),
        "utf8",
      ),
    catch: (cause) => new PolicyGenerationError({ cause }),
  }),
);
