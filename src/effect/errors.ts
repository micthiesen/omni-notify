import { Data } from "effect";

/** A failure raised while adapting a non-Effect API at an infrastructure edge. */
export class IntegrationError extends Data.TaggedError("IntegrationError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}

/** A failure raised while adapting synchronous persistence code. */
export class PersistenceError extends Data.TaggedError("PersistenceError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}

export class HttpResponseError extends Data.TaggedError("HttpResponseError")<{
  readonly statusCode: number;
  readonly body?: string;
}> {
  public override get message(): string {
    return `HTTP ${this.statusCode}${this.body ? `: ${this.body}` : ""}`;
  }
}

const TRANSIENT_NETWORK_CODES = new Set([
  "ECONNRESET",
  "ECONNREFUSED",
  "EHOSTUNREACH",
  "ENETUNREACH",
  "ENOTFOUND",
  "EPIPE",
  "ETIMEDOUT",
]);

/** Retry only transport failures, rate limits, and server-side HTTP failures. */
export function isTransientHttpError(error: unknown): boolean {
  if (typeof error !== "object" || error === null) return false;
  const value = error as Record<string, unknown>;
  const response =
    typeof value.response === "object" && value.response !== null
      ? (value.response as Record<string, unknown>)
      : undefined;
  const status =
    typeof value.statusCode === "number"
      ? value.statusCode
      : typeof value.status === "number"
        ? value.status
        : typeof response?.statusCode === "number"
          ? response.statusCode
          : typeof response?.status === "number"
            ? response.status
            : undefined;
  if (status !== undefined) return status === 429 || status >= 500;
  if (typeof value.code === "string" && TRANSIENT_NETWORK_CODES.has(value.code)) {
    return true;
  }
  if (value.name === "RequestError" || value.name === "TimeoutError") return true;
  return "cause" in value ? isTransientHttpError(value.cause) : false;
}
