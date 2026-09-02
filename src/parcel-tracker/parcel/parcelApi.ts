import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { codeBlock } from "@micthiesen/mitools/markdown";
import { Clock, Effect } from "effect";
import got from "got";
import { ParcelSubmissionError } from "../effect.js";

const API_URL = "https://api.parcel.app/external/add-delivery/";

export type SubmitResult =
  | { status: "success" }
  | { status: "rejected"; statusCode: number }
  | { status: "error" };

// Rejections that indicate an auth or rate-limit problem, not a wrong carrier pick.
const NON_CARRIER_REJECTION_CODES = new Set([401, 403, 429]);

/**
 * Whether a failed submission plausibly indicates the wrong carrier was picked,
 * making it worth retrying with the next ranked candidate. 4xx rejections
 * (except auth/rate-limit) fit — Parcel validates tracking-number/carrier pairs.
 * Network failures and 5xx are transient, so retrying another carrier would only
 * mask the real error.
 */
export function shouldTryNextCandidate(result: SubmitResult): boolean {
  return (
    result.status === "rejected" && !NON_CARRIER_REJECTION_CODES.has(result.statusCode)
  );
}

function httpFailureDetails(cause: unknown): {
  statusCode?: number;
  body: unknown;
} {
  if (typeof cause !== "object" || cause === null) {
    return { body: "no response body" };
  }
  const response = (cause as { response?: unknown }).response;
  if (typeof response !== "object" || response === null) {
    return { body: "no response body" };
  }
  const value = response as { statusCode?: unknown; body?: unknown };
  return {
    statusCode: typeof value.statusCode === "number" ? value.statusCode : undefined,
    body: value.body ?? "no response body",
  };
}

export function submitDeliveryEffect(
  params: {
    trackingNumber: string;
    carrierCode: string;
    description: string;
  },
  apiKey: string,
  logger: Logger,
  rejectionLog?: LogFile,
): Effect.Effect<SubmitResult, never> {
  const payload = {
    tracking_number: params.trackingNumber,
    carrier_code: params.carrierCode,
    description: params.description,
    send_push_confirmation: true,
  };

  logger.info(`Submitting delivery: ${JSON.stringify(payload)}`);

  return Effect.tryPromise({
    try: () =>
      got.post(API_URL, {
        headers: { "api-key": apiKey },
        json: payload,
        timeout: { request: 10_000 },
      }),
    catch: (cause) => new ParcelSubmissionError({ cause }),
  }).pipe(
    Effect.map((response): SubmitResult => {
      logger.info(
        `Submitted delivery: ${params.trackingNumber} (${params.carrierCode}) → ${response.statusCode}`,
      );
      return { status: "success" } satisfies SubmitResult;
    }),
    Effect.catch((error) =>
      Effect.gen(function* () {
        const { statusCode, body } = httpFailureDetails(error.cause);
        logger.error(
          `Failed to submit delivery ${params.trackingNumber}`,
          `${error.message}\nResponse: ${body}`,
        );
        if (statusCode && statusCode >= 400 && statusCode < 500) {
          const now = yield* Clock.currentTimeMillis;
          rejectionLog?.section(
            `Rejected: ${params.trackingNumber} (${statusCode}) — ${new Date(now).toISOString()}`,
            `**Request:**\n${codeBlock(JSON.stringify(payload, null, 2), "json")}\n\n**Response:**\n${codeBlock(String(body))}`,
          );
          return { status: "rejected", statusCode } satisfies SubmitResult;
        }
        return { status: "error" } satisfies SubmitResult;
      }),
    ),
  );
}
