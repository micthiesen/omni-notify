import { describe, expect, it } from "vitest";
import { HttpResponseError, IntegrationError, isTransientHttpError } from "./errors.js";

describe("isTransientHttpError", () => {
  it.each([429, 500, 503])("retries HTTP %i", (statusCode) => {
    expect(isTransientHttpError(new HttpResponseError({ statusCode }))).toBe(true);
  });

  it.each([400, 401, 404, 422])("does not retry HTTP %i", (statusCode) => {
    expect(isTransientHttpError(new HttpResponseError({ statusCode }))).toBe(false);
  });

  it("finds retryable failures through typed adapter errors", () => {
    const network = Object.assign(new Error("socket reset"), { code: "ECONNRESET" });
    expect(
      isTransientHttpError(
        new IntegrationError({ operation: "request", cause: network }),
      ),
    ).toBe(true);
  });

  it("does not retry aborts or arbitrary programming errors", () => {
    expect(isTransientHttpError(new DOMException("aborted", "AbortError"))).toBe(false);
    expect(isTransientHttpError(new TypeError("bad input"))).toBe(false);
  });
});
