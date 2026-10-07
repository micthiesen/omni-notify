import { Data, Effect, Schema } from "effect";
import { fetchPublicText, PUBLIC_HTTP_USER_AGENT } from "../effect/publicHttp.js";

export const resetText = Schema.String.pipe(Schema.check(Schema.isMaxLength(16_384)));
export const resetIdentifier = Schema.String.pipe(
  Schema.check(Schema.isMaxLength(256)),
);
export const resetTimestamp = Schema.String.pipe(
  Schema.check(Schema.makeFilter((value) => Number.isFinite(Date.parse(value)))),
);
export const resetUrl = Schema.String.pipe(
  Schema.check(Schema.isMaxLength(512)),
  Schema.check(
    Schema.makeFilter((value) => {
      try {
        const parsed = new URL(value);
        return parsed.protocol === "https:" && !parsed.username && !parsed.password;
      } catch {
        return false;
      }
    }),
  ),
);

export class ResetSourceError extends Data.TaggedError("ResetSourceError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    return `${this.operation}: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

export function readResetSource<A, I>(url: string, schema: Schema.Codec<A, I>) {
  return fetchPublicText(
    url,
    {
      headers: { "user-agent": PUBLIC_HTTP_USER_AGENT, accept: "application/json" },
      timeout: { request: 20_000 },
      retry: { limit: 0 },
    },
    `fetch reset source ${url}`,
    undefined,
    2 * 1024 * 1024,
  ).pipe(
    Effect.flatMap((body) =>
      Effect.try({
        try: () => JSON.parse(body) as unknown,
        catch: (cause) => new ResetSourceError({ operation: `parse ${url}`, cause }),
      }),
    ),
    Effect.flatMap(Schema.decodeUnknownEffect(schema)),
    Effect.mapError(
      (cause) => new ResetSourceError({ operation: `read ${url}`, cause }),
    ),
  );
}
