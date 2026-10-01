import { createHash } from "node:crypto";
import { Clock, Data, Effect, Schema } from "effect";
import { readFetchResponseTextWithLimit } from "../../effect/publicHttp.js";

export class ExecutorEventAuthorizationError extends Data.TaggedError(
  "ExecutorEventAuthorizationError",
)<{}> {
  override get message() {
    return "Executor authorization check unavailable";
  }
}

const SessionSchema = Schema.Struct({
  userId: Schema.String,
  clientId: Schema.String,
  accessTokenExpiresAt: Schema.Union([Schema.String, Schema.Number]),
});

export const executorOwnerId = (userId: string, clientId: string): string =>
  `executor:${createHash("sha256")
    .update(JSON.stringify([userId, clientId]))
    .digest("hex")}`;

/** Returns only an authorization decision. The session response may contain credentials. */
export function createExecutorEventAuthorizer(
  sessionUrl: string,
  request: typeof fetch = fetch,
): (owner: string, authorization: string) => Effect.Effect<boolean, Error> {
  const endpoint = new URL(sessionUrl);
  if (
    !["http:", "https:"].includes(endpoint.protocol) ||
    endpoint.username ||
    endpoint.password
  ) {
    throw new Error("Invalid Executor session URL");
  }
  return (owner, authorization) =>
    Effect.gen(function* () {
      const now = yield* Clock.currentTimeMillis;
      return yield* Effect.tryPromise({
        try: async (signal) => {
          if (!/^executor:[0-9a-f]{64}$/.test(owner)) return false;
          if (!/^Bearer [^\s]{1,8192}$/.test(authorization)) return false;
          const response = await request(endpoint, {
            method: "GET",
            headers: {
              Authorization: authorization,
              "User-Agent": "OpenAI File Downloader, XaiImageApiFetch/1.0",
            },
            redirect: "error",
            signal: AbortSignal.any([signal, AbortSignal.timeout(10_000)]),
          });
          if (response.status === 401 || response.status === 403) {
            await response.body?.cancel();
            return false;
          }
          if (!response.ok) {
            await response.body?.cancel();
            throw new ExecutorEventAuthorizationError();
          }
          const raw: unknown = JSON.parse(
            await readFetchResponseTextWithLimit(response, 64 * 1024, signal),
          );
          let session: Schema.Schema.Type<typeof SessionSchema>;
          try {
            session = Schema.decodeUnknownSync(SessionSchema)(raw);
          } catch {
            return false;
          }
          const expiry =
            typeof session.accessTokenExpiresAt === "number"
              ? session.accessTokenExpiresAt
              : Date.parse(session.accessTokenExpiresAt);
          return (
            Number.isFinite(expiry) &&
            expiry > now &&
            executorOwnerId(session.userId, session.clientId) === owner
          );
        },
        catch: () => new ExecutorEventAuthorizationError(),
      });
    });
}
