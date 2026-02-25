import type { Logger } from "@micthiesen/mitools/logging";
import { generateText, Output } from "ai";
import { getExtractionModel } from "../../ai/registry.js";
import { getCarrierCodesForPrompt } from "../carriers/carrierMap.js";
import { deliveryExtractionSchema } from "./schema.js";

const MAX_BODY_CHARS = 3000;

export async function extractDeliveries(
  email: { subject: string; from: string; textBody: string },
  logger: Logger,
): Promise<{ tracking_number: string; carrier_code: string; description: string }[]> {
  const { model, modelId } = getExtractionModel();
  const carrierCodes = await getCarrierCodesForPrompt(logger);
  const body = email.textBody.slice(0, MAX_BODY_CHARS);

  const prompt = `Extract package tracking numbers from this email. If no tracking numbers are found, return an empty deliveries array.

For carrier_code, use the short code (left of the colon) from this list — not the display name. If the carrier is not in this list, omit that tracking number entirely.
${carrierCodes}

From: ${email.from}
Subject: ${email.subject}

${body}`;

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
