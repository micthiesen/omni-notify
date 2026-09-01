import type { Logger } from "@micthiesen/mitools/logging";
import { JamClient } from "jmap-jam";
import { Data, Effect } from "effect";

const SESSION_URL = "https://api.fastmail.com/jmap/session";

export interface JmapContext {
  jam: JamClient;
  accountId: string;
}

export class JmapClientError extends Data.TaggedError("JmapClientError")<{
  readonly cause: unknown;
}> {
  public override get message(): string {
    return this.cause instanceof Error ? this.cause.message : String(this.cause);
  }
}

export function createJmapClientEffect(
  bearerToken: string,
  logger: Logger,
): Effect.Effect<JmapContext, JmapClientError> {
  return Effect.gen(function* () {
    const jam = new JamClient({ sessionUrl: SESSION_URL, bearerToken });

    const accountId = yield* Effect.tryPromise({
      try: () => jam.getPrimaryAccount(),
      catch: (cause) => new JmapClientError({ cause }),
    });
    if (!accountId) {
      return yield* new JmapClientError({
        cause: new Error("Could not resolve primary mail account from JMAP session"),
      });
    }

    logger.info(`JMAP session established (account: ${accountId})`);
    return { jam, accountId };
  });
}
