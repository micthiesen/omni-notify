import type { Logger } from "@micthiesen/mitools/logging";
import got, { type HTTPError } from "got";

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
    return true;
  } catch (error) {
    const body = (error as HTTPError).response?.body ?? "no response body";
    logger.error(
      `Failed to submit delivery ${params.trackingNumber}`,
      `${(error as Error).message}\nResponse: ${body}`,
    );
    return false;
  }
}
