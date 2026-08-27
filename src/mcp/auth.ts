import { createHash, timingSafeEqual } from "node:crypto";

export const MIN_MCP_TOKEN_LENGTH = 32;
const MIN_DISTINCT_TOKEN_CHARS = 12;

export function isStrongMcpToken(token: string): boolean {
  if (token.length < MIN_MCP_TOKEN_LENGTH) return false;
  if (/\s/.test(token)) return false;
  return new Set(token).size >= MIN_DISTINCT_TOKEN_CHARS;
}

/** Fail fast in production; development may leave MCP disabled. */
export function validateMcpTokenConfiguration(
  token: string | undefined,
  production: boolean,
): void {
  if (!production && token === undefined) return;
  if (!token) {
    throw new Error("OMNI_MCP_TOKEN is required in production");
  }
  if (!isStrongMcpToken(token)) {
    throw new Error(
      `OMNI_MCP_TOKEN must be at least ${MIN_MCP_TOKEN_LENGTH} characters with sufficient character diversity`,
    );
  }
}

function tokenDigest(value: string): Buffer {
  return createHash("sha256").update(value, "utf8").digest();
}

/** Hash both values first so comparison time does not reveal token length. */
export function hasValidBearerToken(
  authorization: string | undefined,
  expectedToken: string,
): boolean {
  const match = authorization?.match(/^Bearer ([^\s]+)$/i);
  const supplied = match?.[1] ?? "";
  return timingSafeEqual(tokenDigest(supplied), tokenDigest(expectedToken));
}

export function unauthorizedMcpResponse(): Response {
  return Response.json(
    { error: "Unauthorized" },
    {
      status: 401,
      headers: {
        "Cache-Control": "no-store",
        "WWW-Authenticate": "Bearer",
      },
    },
  );
}
