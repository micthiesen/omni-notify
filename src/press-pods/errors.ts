import { APICallError } from "ai";

/**
 * Whether a pipeline failure is worth an automatic backoff retry. Covers
 * Mistral SDK errors (statusCode / network error names), AI SDK provider
 * errors, and generic Node network failures. Everything else (bad article,
 * extraction failure, programming errors) fails permanently — the job stays
 * visible in the UI with a manual retry.
 */
export function isRetryableError(error: unknown): boolean {
  if (!(error instanceof Error)) return false;

  if (APICallError.isInstance(error)) return error.isRetryable;

  // Mistral SDK: MistralError (has statusCode) for HTTP errors
  if ("statusCode" in error && typeof error.statusCode === "number") {
    const status = error.statusCode;
    return status === 429 || (status >= 500 && status <= 599);
  }

  // got HTTPError: the status lives on the response rather than the error
  // itself. Keep this structural so the generic pipeline error classifier does
  // not need to depend on got's concrete classes.
  if (
    "response" in error &&
    error.response !== null &&
    typeof error.response === "object" &&
    "statusCode" in error.response &&
    typeof error.response.statusCode === "number"
  ) {
    const status = error.response.statusCode;
    return status === 429 || (status >= 500 && status <= 599);
  }

  // Mistral SDK: HTTPClientError subclasses for network failures
  const retryableNames = [
    "ConnectionError",
    "RequestTimeoutError",
    "UnexpectedClientError",
  ];
  if (retryableNames.includes(error.name)) return true;

  // Node/got network failures
  const retryableCodes = [
    "ECONNRESET",
    "ECONNREFUSED",
    "ETIMEDOUT",
    "EAI_AGAIN",
    "ENOTFOUND",
  ];
  if ("code" in error && retryableCodes.includes(String(error.code))) return true;

  return false;
}

export function summarizeError(error: unknown): string {
  if (!(error instanceof Error)) return String(error);

  if ("statusCode" in error && typeof error.statusCode === "number") {
    const body =
      "body" in error && typeof error.body === "string" ? error.body.slice(0, 200) : "";
    return `${error.statusCode}: ${body || error.message}`;
  }

  return `${error.name}: ${error.message}`.slice(0, 300);
}
