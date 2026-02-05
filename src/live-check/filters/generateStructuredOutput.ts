import type { LanguageModel } from "ai";
import { generateText, Output } from "ai";
import type { z } from "zod";

export type GenerateStructuredOutputOptions<T extends z.ZodType> = {
  model: LanguageModel;
  schema: T;
  system?: string;
  prompt: string;
  maxRetries?: number;
  onRetry?: (attempt: number, error: unknown) => void;
};

export type GenerateStructuredOutputResult<T extends z.ZodType> = {
  output: z.infer<T>;
  attempts: number;
};

/**
 * Generates structured output from an LLM with automatic retries on schema validation failures.
 *
 * The AI SDK's built-in `maxRetries` only handles network/API errors, not schema validation
 * failures. This utility adds retry logic for when the LLM responds but the output doesn't
 * match the expected schema.
 */
export async function generateStructuredOutput<T extends z.ZodType>({
  model,
  schema,
  system,
  prompt,
  maxRetries = 2,
  onRetry,
}: GenerateStructuredOutputOptions<T>): Promise<GenerateStructuredOutputResult<T>> {
  let lastError: unknown;

  for (let attempt = 1; attempt <= maxRetries + 1; attempt++) {
    try {
      const { output } = await generateText({
        model,
        output: Output.object({ schema }),
        system,
        prompt,
      });

      if (!output) {
        throw new Error("LLM returned no structured output");
      }

      return { output, attempts: attempt };
    } catch (error) {
      lastError = error;

      if (attempt <= maxRetries) {
        onRetry?.(attempt, error);
      }
    }
  }

  throw lastError;
}
