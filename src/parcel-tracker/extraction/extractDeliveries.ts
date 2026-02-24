import type { Logger } from "@micthiesen/mitools/logging";
import { generateText, Output } from "ai";
import { getExtractionModel } from "../../ai/registry.js";
import { deliveryExtractionSchema } from "./schema.js";

export async function extractDeliveries(
  email: { subject: string; from: string; textBody: string },
  logger: Logger,
): Promise<{ tracking_number: string; carrier: string; description: string }[]> {
  const { model, modelId } = getExtractionModel();
  logger.debug(`Extracting deliveries with ${modelId}`);

  const result = await generateText({
    model,
    output: Output.object({ schema: deliveryExtractionSchema }),
    prompt: `Extract package tracking numbers from this email. If no tracking numbers are found, return an empty deliveries array.

From: ${email.from}
Subject: ${email.subject}

${email.textBody}`,
  });

  return result.output?.deliveries ?? [];
}
