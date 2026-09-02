import { HTTPError } from "got";

/** Retained while PressPods still uses got directly. */
export function extractHttpError(error: unknown): unknown {
  if (!(error instanceof HTTPError)) return error;
  const { response } = error;
  let body: unknown = response.body;
  if (typeof body === "string") {
    try {
      body = JSON.parse(body);
    } catch {
      // Keep the original response text.
    }
  }
  return {
    statusCode: response.statusCode,
    statusMessage: response.statusMessage ?? "",
    body,
    url: response.url,
    method: response.request?.options?.method ?? "UNKNOWN",
  };
}
