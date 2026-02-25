import type { Logger } from "@micthiesen/mitools/logging";
import { generateText, Output } from "ai";
import { getExtractionModel } from "../../ai/registry.js";
import { deliveryExtractionSchema } from "./schema.js";

export async function extractDeliveries(
  email: { subject: string; from: string; textBody: string },
  logger: Logger,
): Promise<{ tracking_number: string; carrier: string; description: string }[]> {
  const { model, modelId } = getExtractionModel();

  const prompt = `Extract package tracking numbers from this email. If no tracking numbers are found, return an empty deliveries array.

From: ${email.from}
Subject: ${email.subject}

${email.textBody}`;

  logger.info(`Extraction prompt (${modelId}):\n${prompt}`);

  const result = await generateText({
    model,
    output: Output.object({ schema: deliveryExtractionSchema }),
    prompt,
  });

  logger.info(`Extraction response: ${JSON.stringify(result.output)}`);
  logger.info(
    `Token usage: ${result.usage.inputTokens} prompt, ${result.usage.outputTokens} completion`,
  );

  return result.output?.deliveries ?? [];
}
