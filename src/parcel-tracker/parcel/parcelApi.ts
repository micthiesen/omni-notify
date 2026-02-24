import type { Logger } from "@micthiesen/mitools/logging";
import got from "got";

const API_URL = "https://api.parcel.app/external/add-delivery/";

export async function submitDelivery(
  params: {
    trackingNumber: string;
    carrierCode: string;
    description: string;
  },
  apiKey: string,
  logger: Logger,
): Promise<boolean> {
  try {
    await got.post(API_URL, {
      headers: { "api-key": apiKey },
      json: {
        tracking_number: params.trackingNumber,
        carrier_code: params.carrierCode,
        description: params.description,
        send_push_confirmation: true,
      },
      timeout: { request: 10_000 },
    });

    logger.info(`Submitted delivery: ${params.trackingNumber} (${params.carrierCode})`);
    return true;
  } catch (error) {
    logger.error(
      `Failed to submit delivery ${params.trackingNumber}`,
      (error as Error).message,
    );
    return false;
  }
}
