import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import got, { type HTTPError } from "got";
import { codeBlock } from "../../utils/markdown.js";

const API_URL = "https://api.parcel.app/external/add-delivery/";

export type SubmitResult =
  | { status: "success" }
  | { status: "rejected"; statusCode: number }
  | { status: "error" };

export async function submitDelivery(
  params: {
    trackingNumber: string;
    carrierCode: string;
    description: string;
  },
  apiKey: string,
  logger: Logger,
  rejectionLog?: LogFile,
): Promise<SubmitResult> {
  const payload = {
    tracking_number: params.trackingNumber,
    carrier_code: params.carrierCode,
    description: params.description,
    send_push_confirmation: true,
  };

  logger.info(`Submitting delivery: ${JSON.stringify(payload)}`);

  try {
    const response = await got.post(API_URL, {
      headers: { "api-key": apiKey },
      json: payload,
      timeout: { request: 10_000 },
    });

    logger.info(
      `Submitted delivery: ${params.trackingNumber} (${params.carrierCode}) → ${response.statusCode}`,
    );
    return { status: "success" };
  } catch (error) {
    const statusCode = (error as HTTPError).response?.statusCode;
    const body = (error as HTTPError).response?.body ?? "no response body";
    logger.error(
      `Failed to submit delivery ${params.trackingNumber}`,
      `${(error as Error).message}\nResponse: ${body}`,
    );
    if (statusCode && statusCode >= 400 && statusCode < 500) {
      rejectionLog?.section(
        `Rejected: ${params.trackingNumber} (${statusCode}) — ${new Date().toISOString()}`,
        `**Request:**\n${codeBlock(JSON.stringify(payload, null, 2), "json")}\n\n**Response:**\n${codeBlock(String(body))}`,
      );
      return { status: "rejected", statusCode };
    }
    return { status: "error" };
  }
}
