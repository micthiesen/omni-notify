import { describe, expect, it } from "vitest";
import { isRetryableError, summarizeError } from "./errors.js";

function withStatus(status: number): Error {
  const error = new Error(`HTTP ${status}`);
  Object.assign(error, { statusCode: status });
  return error;
}

function withResponseStatus(status: number): Error {
  const error = new Error(`HTTP ${status}`);
  Object.assign(error, { response: { statusCode: status } });
  return error;
}

describe("isRetryableError", () => {
  it("retries 429 and 5xx statuses", () => {
    expect(isRetryableError(withStatus(429))).toBe(true);
    expect(isRetryableError(withStatus(500))).toBe(true);
    expect(isRetryableError(withStatus(503))).toBe(true);
  });

  it("does not retry 4xx client errors", () => {
    expect(isRetryableError(withStatus(400))).toBe(false);
    expect(isRetryableError(withStatus(401))).toBe(false);
  });

  it("recognizes got response status codes", () => {
    expect(isRetryableError(withResponseStatus(429))).toBe(true);
    expect(isRetryableError(withResponseStatus(503))).toBe(true);
    expect(isRetryableError(withResponseStatus(400))).toBe(false);
  });

  it("retries known network error names", () => {
    const error = new Error("connect failed");
    error.name = "ConnectionError";
    expect(isRetryableError(error)).toBe(true);
  });

  it("retries Node network error codes", () => {
    const error = new Error("reset");
    Object.assign(error, { code: "ECONNRESET" });
    expect(isRetryableError(error)).toBe(true);
  });

  it("does not retry plain errors or non-errors", () => {
    expect(isRetryableError(new Error("nope"))).toBe(false);
    expect(isRetryableError("string")).toBe(false);
  });
});

describe("summarizeError", () => {
  it("includes status code and body when present", () => {
    const error = withStatus(500);
    Object.assign(error, { body: "internal" });
    expect(summarizeError(error)).toBe("500: internal");
  });

  it("falls back to name and message", () => {
    expect(summarizeError(new Error("kaput"))).toBe("Error: kaput");
  });
});
