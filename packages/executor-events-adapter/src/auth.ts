import { createHash } from "node:crypto";
import { Effect, Option, Schema } from "effect";

export interface AdapterOptions {
  readonly executorBaseUrl: string;
  readonly omniBaseUrl: string;
  readonly omniMcpToken: string;
  readonly allowedUserId: string;
  readonly publicMcpOrigin: string;
}

export interface AuthenticatedOwner {
  readonly owner: string;
  readonly authorization: string;
  readonly expiresAt: number;
}

const BEARER = /^Bearer ([A-Za-z0-9._~+/-]+=*)$/;
const Session = Schema.Struct({
  userId: Schema.String,
  clientId: Schema.String,
  accessTokenExpiresAt: Schema.String,
});

export function ownerId(userId: string, clientId: string): string {
  return `executor:${createHash("sha256")
    .update(JSON.stringify([userId, clientId]))
    .digest("hex")}`;
}

/** Better Auth's get-session does not check expiry, so this adapter must. */
export function authenticateOAuth(
  authorization: string | undefined,
  options: AdapterOptions,
) {
  if (!authorization || !BEARER.test(authorization))
    return Effect.succeed<AuthenticatedOwner | null>(null);
  return Effect.tryPromise({
    try: async () => {
      const response = await fetch(
        new URL("/api/auth/mcp/get-session", options.executorBaseUrl),
        {
          headers: {
            authorization,
            "user-agent": "OpenAI File Downloader, XaiImageApiFetch/1.0",
          },
          redirect: "error",
          signal: AbortSignal.timeout(5000),
        },
      );
      if (!response.ok) throw new Error("Executor authentication unavailable");
      return response.json() as Promise<unknown>;
    },
    catch: () => new Error("Executor authentication unavailable"),
  }).pipe(
    Effect.map((raw): AuthenticatedOwner | null => {
      const decoded = Schema.decodeUnknownOption(Session)(raw);
      if (Option.isNone(decoded)) return null;
      const session = decoded.value;
      if (!session.userId || !session.clientId) return null;
      const expiresAt = Date.parse(session.accessTokenExpiresAt);
      if (!Number.isFinite(expiresAt) || expiresAt <= Date.now()) return null;
      if (session.userId !== options.allowedUserId) return null;
      return {
        owner: ownerId(session.userId, session.clientId),
        authorization,
        expiresAt,
      };
    }),
  );
}
