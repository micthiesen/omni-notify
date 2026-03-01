import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { generateText, Output } from "ai";
import { getExtractionModel } from "../../ai/registry.js";
import { getCarrierCodesForPrompt } from "../carriers/carrierMap.js";
import { deliveryExtractionSchema } from "./schema.js";

const MAX_BODY_CHARS = 3000;

export async function extractDeliveries(
  email: { subject: string; from: string; textBody: string },
  logger: Logger,
  logFile?: LogFile,
): Promise<{ tracking_number: string; carrier_code: string; description: string }[]> {
  const { model, modelId } = getExtractionModel();
  const carrierCodes = await getCarrierCodesForPrompt(logger);
  const body = email.textBody.slice(0, MAX_BODY_CHARS);

  const prompt = `Extract package tracking numbers from this email. If no tracking numbers are found, return an empty deliveries array.

For carrier_code, use the short code (left of the colon) from this list — not the display name. If the carrier is not in this list, omit that tracking number entirely.
${carrierCodes}

Carrier guidance:
- Dragonfly: Always use carrier code "intelc" (Dragonfly is Intelcom's brand).
- Tracking numbers starting with "JY" (e.g. JY25CA10A002279541): Use carrier code "uniuni". These are UniUni last-mile deliveries, often from AliExpress shipments. Prefer "uniuni" over any AliExpress carrier.

From: ${email.from}
Subject: ${email.subject}

${body}`;

  if (logFile) {
    logFile.log(logger, LogLevel.INFO, `Extraction Prompt (${modelId})`, prompt, {
      consoleSummary: `Extraction prompt (${modelId}) [${prompt.length} chars]`,
    });
  } else {
    logger.info(`Extraction prompt (${modelId}):\n${prompt}`);
  }

  const result = await generateText({
    model,
    output: Output.object({ schema: deliveryExtractionSchema }),
    prompt,
  });

  const response = JSON.stringify(result.output);
  if (logFile) {
    logFile.log(logger, LogLevel.INFO, "Extraction Response", response);
  } else {
    logger.info(`Extraction response: ${response}`);
  }
  logger.info(
    `Token usage: ${result.usage.inputTokens} prompt, ${result.usage.outputTokens} completion`,
  );

  return result.output?.deliveries ?? [];
}
