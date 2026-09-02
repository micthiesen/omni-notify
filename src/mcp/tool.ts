import type { Effect as EffectType } from "effect/Effect";
import type { CallToolResult, ToolAnnotations } from "@modelcontextprotocol/server";
import { Cause, Data, Effect } from "effect";
import { z } from "zod";
import type { AppServices } from "../effect/appRuntime.js";

export type ExecutorPolicy = "allow" | "require_approval" | "block";

export interface ToolPolicy {
  /** Concrete effects beyond returning the result. */
  sideEffects: string[];
  /** Monetary cost or quota consumption expected from one call. */
  cost: string;
  /** Recommended Executor policy; MCP annotations are descriptive only. */
  recommendedPolicy: ExecutorPolicy;
}

export interface McpToolDefinition<R = AppServices> {
  name: string;
  title: string;
  description: string;
  inputSchema: z.ZodType<Record<string, unknown>>;
  outputSchema: z.ZodType<Record<string, unknown>>;
  annotations: Required<
    Pick<
      ToolAnnotations,
      "readOnlyHint" | "destructiveHint" | "idempotentHint" | "openWorldHint"
    >
  >;
  policy: ToolPolicy;
  execute: (
    input: Record<string, unknown>,
  ) => EffectType<Record<string, unknown>, McpToolError, R>;
}

export class McpToolError extends Data.TaggedError("McpToolError")<{
  readonly tool: string;
  readonly phase: "input" | "execute" | "output";
  readonly cause: unknown;
}> {
  public override get message(): string {
    return toolErrorMessage(this.cause);
  }
}

function toolErrorMessage(cause: unknown): string {
  if (
    typeof cause === "object" &&
    cause !== null &&
    "cause" in cause &&
    cause.cause !== cause
  ) {
    return toolErrorMessage(cause.cause);
  }
  return cause instanceof Error ? cause.message : String(cause);
}

type ToolDefinitionInput<
  TInput extends z.ZodType<Record<string, unknown>>,
  TError,
  R,
> = Omit<McpToolDefinition<R>, "inputSchema" | "outputSchema" | "execute"> & {
  inputSchema: TInput;
  outputSchema: z.ZodType<Record<string, unknown>>;
  execute: (input: z.output<TInput>) => EffectType<Record<string, unknown>, TError, R>;
};

export function defineTool<
  TInput extends z.ZodType<Record<string, unknown>>,
  TError,
  R,
>(definition: ToolDefinitionInput<TInput, TError, R>): McpToolDefinition<R> {
  const execute = (input: Record<string, unknown>) =>
    Effect.gen(function* () {
      const decodedInput = yield* Effect.try({
        try: () => definition.inputSchema.parse(input),
        catch: (cause) =>
          new McpToolError({ tool: definition.name, phase: "input", cause }),
      });
      const value = yield* definition.execute(decodedInput).pipe(
        Effect.catchCause((cause) =>
          Effect.fail(
            new McpToolError({
              tool: definition.name,
              phase: "execute",
              cause: Cause.squash(cause),
            }),
          ),
        ),
      );
      return yield* Effect.try({
        try: () => definition.outputSchema.parse(value),
        catch: (cause) =>
          new McpToolError({ tool: definition.name, phase: "output", cause }),
      });
    });

  return { ...definition, execute } as McpToolDefinition<R>;
}

export const annotations = (
  readOnlyHint: boolean,
  destructiveHint: boolean,
  idempotentHint: boolean,
  openWorldHint: boolean,
): McpToolDefinition["annotations"] => ({
  readOnlyHint,
  destructiveHint,
  idempotentHint,
  openWorldHint,
});

export function successfulToolResult(value: Record<string, unknown>): CallToolResult {
  return {
    content: [{ type: "text", text: JSON.stringify(value) }],
    structuredContent: value,
  };
}

export function failedToolResult(error: unknown): CallToolResult {
  const message = error instanceof Error ? error.message : "Tool call failed";
  return {
    isError: true,
    content: [{ type: "text", text: message }],
  };
}

export const emptyInputSchema = z.object({}).strict();

export const paginationInputShape = {
  cursor: z.number().int().min(0).default(0).describe("Zero-based result offset"),
  limit: z.number().int().min(1).max(100).default(25),
};

export function paginate<T>(
  values: T[],
  cursor: number,
  limit: number,
): { items: T[]; nextCursor: number | null; total: number } {
  const items = values.slice(cursor, cursor + limit);
  const next = cursor + items.length;
  return {
    items,
    nextCursor: next < values.length ? next : null,
    total: values.length,
  };
}

export function truncate(
  value: string,
  maxChars: number,
): {
  text: string;
  truncated: boolean;
} {
  return {
    text: value.slice(0, maxChars),
    truncated: value.length > maxChars,
  };
}
