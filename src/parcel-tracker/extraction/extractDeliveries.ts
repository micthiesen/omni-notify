import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { codeBlock } from "@micthiesen/mitools/markdown";
import { generateText, Output } from "ai";
import { getExtractionModel } from "../../ai/registry.js";
import { MAX_CARRIER_CANDIDATES } from "../carriers/candidates.js";
import { getCarrierCodesForPrompt } from "../carriers/carrierMap.js";
import { deliveryExtractionSchema } from "./schema.js";

const MAX_BODY_CHARS = 3000;

export interface ExtractedDelivery {
  tracking_number: string;
  carrier_candidates: string[];
  description: string;
}

export async function extractDeliveries(
  email: { subject: string; from: string; textBody: string },
  logger: Logger,
  logFile?: LogFile,
): Promise<ExtractedDelivery[]> {
  const { model, modelId } = getExtractionModel();
  const carrierCodes = await getCarrierCodesForPrompt(logger);
  const body = email.textBody.slice(0, MAX_BODY_CHARS);

  const prompt = `Extract package tracking numbers from this email. If no tracking numbers are found, return an empty deliveries array.

For carrier_candidates, list up to ${MAX_CARRIER_CANDIDATES} plausible carrier codes ranked most likely first, using the short code (left of the colon) from this list — not the display name. If no carrier in this list could plausibly match, omit that tracking number entirely.
${carrierCodes}

Carrier guidance:
- The recipient is in Canada. When a carrier brand has entries for multiple countries, rank the Canadian entry first for domestic shipments (e.g. "dicom" for GLS Canada ahead of "gls" for GLS Europe) and include the other regional variants only as lower-ranked candidates.
- Dragonfly: Always use carrier code "intelc" (Dragonfly is Intelcom's brand).
- Tracking numbers starting with "JY" (e.g. JY25CA10A002279541): Use carrier code "uniuni". These are UniUni last-mile deliveries, often from AliExpress shipments. Prefer "uniuni" over any AliExpress carrier.

From: ${email.from}
Subject: ${email.subject}

${body}`;

  if (logFile) {
    logFile.log(
      logger,
      LogLevel.INFO,
      `Extraction Prompt (${modelId})`,
      codeBlock(prompt),
      { consoleSummary: `Extraction prompt (${modelId}) [${prompt.length} chars]` },
    );
  } else {
    logger.info(`Extraction prompt (${modelId}):\n${prompt}`);
  }

  const result = await generateText({
    model,
    output: Output.object({ schema: deliveryExtractionSchema }),
    prompt,
  });

  const response = JSON.stringify(result.output, null, 2);
  if (logFile) {
    logFile.log(
      logger,
      LogLevel.INFO,
      "Extraction Response",
      codeBlock(response, "json"),
    );
  } else {
    logger.info(`Extraction response: ${response}`);
  }
  logger.info(
    `Token usage: ${result.usage.inputTokens} prompt, ${result.usage.outputTokens} completion`,
  );

  const deliveries = result.output?.deliveries ?? [];
  return deliveries.map((delivery) => ({
    ...delivery,
    carrier_candidates: delivery.carrier_candidates.slice(0, MAX_CARRIER_CANDIDATES),
  }));
}
