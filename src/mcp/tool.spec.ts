import { Cause, Effect, Exit, Option } from "effect";
import { describe, expect, it } from "vitest";
import { z } from "zod";
import { annotations, defineTool, McpToolError } from "./tool.js";

const policy = {
  sideEffects: [] as string[],
  cost: "none",
  recommendedPolicy: "allow" as const,
};

describe("MCP Effect tool contract", () => {
  it("executes as an Effect and preserves structured output", async () => {
    const tool = defineTool({
      name: "effect_test",
      title: "Effect Test",
      description: "Exercises the canonical Effect MCP tool contract.",
      inputSchema: z.object({ value: z.string() }).strict(),
      outputSchema: z.object({ value: z.string() }),
      annotations: annotations(true, false, true, false),
      policy,
      execute: ({ value }) => Effect.succeed({ value: value.toUpperCase() }),
    });

    await expect(Effect.runPromise(tool.execute({ value: "ok" }))).resolves.toEqual({
      value: "OK",
    });
  });

  it("turns thrown defects into a safe tagged tool failure", async () => {
    const tool = defineTool({
      name: "defect_test",
      title: "Defect Test",
      description: "Exercises safe conversion of a tool implementation defect.",
      inputSchema: z.object({}).strict(),
      outputSchema: z.object({ ok: z.boolean() }),
      annotations: annotations(true, false, true, false),
      policy,
      execute: () =>
        Effect.sync(() => {
          throw new Error("private implementation failure");
        }),
    });

    const exit = await Effect.runPromiseExit(tool.execute({}));
    expect(Exit.isFailure(exit)).toBe(true);
    if (Exit.isFailure(exit)) {
      const failure = Cause.findErrorOption(exit.cause);
      expect(Option.isSome(failure)).toBe(true);
      if (Option.isSome(failure)) expect(failure.value).toBeInstanceOf(McpToolError);
    }
  });

  it("enforces declared input defaults and output validation", async () => {
    const tool = defineTool({
      name: "output_test",
      title: "Output Test",
      description: "Exercises internal protocol schema decoding.",
      inputSchema: z.object({ value: z.string().default("defaulted") }).strict(),
      outputSchema: z.object({ ok: z.boolean() }),
      annotations: annotations(true, false, true, false),
      policy,
      execute: ({ value }) =>
        Effect.succeed({ ok: value } as unknown as Record<string, unknown>),
    });

    const exit = await Effect.runPromiseExit(tool.execute({}));
    expect(Exit.isFailure(exit)).toBe(true);
    if (Exit.isFailure(exit)) {
      expect(Option.isSome(Cause.findErrorOption(exit.cause))).toBe(true);
    }

    const invalidInputExit = await Effect.runPromiseExit(
      tool.execute({ value: "ok", unexpected: true }),
    );
    expect(Exit.isFailure(invalidInputExit)).toBe(true);
    if (Exit.isFailure(invalidInputExit)) {
      const failure = Cause.findErrorOption(invalidInputExit.cause);
      expect(Option.isSome(failure)).toBe(true);
      if (Option.isSome(failure)) expect(failure.value).toBeInstanceOf(McpToolError);
    }
  });
});
