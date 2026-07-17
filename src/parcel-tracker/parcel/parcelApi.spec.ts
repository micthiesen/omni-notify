import { describe, expect, it } from "vitest";
import { shouldTryNextCandidate } from "./parcelApi.js";

describe("shouldTryNextCandidate", () => {
  it("returns true for a 400 rejection (likely wrong carrier)", () => {
    expect(shouldTryNextCandidate({ status: "rejected", statusCode: 400 })).toBe(true);
  });

  it("returns true for other 4xx rejections like 404 and 422", () => {
    expect(shouldTryNextCandidate({ status: "rejected", statusCode: 404 })).toBe(true);
    expect(shouldTryNextCandidate({ status: "rejected", statusCode: 422 })).toBe(true);
  });

  it("returns false for auth rejections (not carrier-related)", () => {
    expect(shouldTryNextCandidate({ status: "rejected", statusCode: 401 })).toBe(false);
    expect(shouldTryNextCandidate({ status: "rejected", statusCode: 403 })).toBe(false);
  });

  it("returns false for rate-limit rejections", () => {
    expect(shouldTryNextCandidate({ status: "rejected", statusCode: 429 })).toBe(false);
  });

  it("returns false for transient errors (network/5xx)", () => {
    expect(shouldTryNextCandidate({ status: "error" })).toBe(false);
  });

  it("returns false for success", () => {
    expect(shouldTryNextCandidate({ status: "success" })).toBe(false);
  });
});
