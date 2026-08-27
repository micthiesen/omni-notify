import { describe, expect, it } from "vitest";
import {
  hasValidBearerToken,
  isStrongMcpToken,
  validateMcpTokenConfiguration,
} from "./auth.js";

const TEST_TOKEN = "test-token-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";

describe("MCP bearer authentication", () => {
  it("accepts only the exact bearer token", () => {
    expect(hasValidBearerToken(`Bearer ${TEST_TOKEN}`, TEST_TOKEN)).toBe(true);
    expect(hasValidBearerToken(`bearer ${TEST_TOKEN}`, TEST_TOKEN)).toBe(true);
    expect(hasValidBearerToken(undefined, TEST_TOKEN)).toBe(false);
    expect(hasValidBearerToken("Basic abc", TEST_TOKEN)).toBe(false);
    expect(hasValidBearerToken("Bearer wrong", TEST_TOKEN)).toBe(false);
    expect(hasValidBearerToken(`Bearer ${TEST_TOKEN} extra`, TEST_TOKEN)).toBe(false);
  });

  it("requires a strong token in production", () => {
    expect(() => validateMcpTokenConfiguration(TEST_TOKEN, true)).not.toThrow();
    expect(() => validateMcpTokenConfiguration(undefined, false)).not.toThrow();
    expect(() => validateMcpTokenConfiguration(undefined, true)).toThrow(
      "OMNI_MCP_TOKEN is required in production",
    );
    expect(() => validateMcpTokenConfiguration("short", true)).toThrow(
      "at least 32 characters",
    );
    expect(isStrongMcpToken("a".repeat(64))).toBe(false);
    expect(isStrongMcpToken(`${TEST_TOKEN}\n`)).toBe(false);
  });
});
