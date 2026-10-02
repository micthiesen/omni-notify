import { Data, Effect, Schema } from "effect";
import { fetchPublicText, PUBLIC_HTTP_USER_AGENT } from "../effect/publicHttp.js";

const text = Schema.String.pipe(Schema.check(Schema.isMaxLength(16_384)));
const identifier = Schema.String.pipe(Schema.check(Schema.isMaxLength(256)));
const timestamp = Schema.String.pipe(
  Schema.check(Schema.makeFilter((value) => Number.isFinite(Date.parse(value)))),
);
const nullableText = Schema.NullOr(text);
const url = Schema.String.pipe(
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

export const AlertFeedSchema = Schema.Struct({
  generatedAt: timestamp,
  items: Schema.Array(
    Schema.Struct({
      id: text,
      eventId: text,
      postId: nullableText,
      topic: text,
      state: text,
      title: text,
      summary: text,
      sourceUrl: url,
      evidenceId: Schema.NullOr(identifier),
      targetAt: Schema.NullOr(timestamp),
      publishedAt: timestamp,
      withdrawn: Schema.Boolean,
    }),
  ).pipe(Schema.check(Schema.isMaxLength(1_000))),
});

export const HistorySchema = Schema.Struct({
  items: Schema.Array(
    Schema.Struct({
      id: text,
      kind: text,
      scope: identifier,
      eventKind: text,
      evidenceClass: text,
      status: text,
      fulfilledBy: nullableText,
      supersededBy: nullableText,
      sources: Schema.Array(Schema.Struct({ announcementId: text, url })),
    }),
  ).pipe(Schema.check(Schema.isMaxLength(5_000))),
});

export type AlertFeed = typeof AlertFeedSchema.Type;
export type ResetHistory = typeof HistorySchema.Type;

export class ResetSourceError extends Data.TaggedError("ResetSourceError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    return `${this.operation}: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

function readSource<A, I>(path: string, schema: Schema.Codec<A, I>) {
  return fetchPublicText(
    `https://resetbeacon.com/api/${path}`,
    {
      headers: { "user-agent": PUBLIC_HTTP_USER_AGENT, accept: "application/json" },
      timeout: { request: 20_000 },
      retry: { limit: 0 },
    },
    `fetch Codex reset ${path}`,
    undefined,
    2 * 1024 * 1024,
  ).pipe(
    Effect.flatMap((body) =>
      Effect.try({
        try: () => JSON.parse(body) as unknown,
        catch: (cause) => new ResetSourceError({ operation: `parse ${path}`, cause }),
      }),
    ),
    Effect.flatMap(Schema.decodeUnknownEffect(schema)),
    Effect.mapError(
      (cause) => new ResetSourceError({ operation: `read ${path}`, cause }),
    ),
  );
}

export const readResetSources = Effect.fn("CodexResets.readSources")(function* () {
  return yield* Effect.all(
    {
      feed: readSource("alerts", AlertFeedSchema),
      history: readSource("history", HistorySchema),
    },
    { concurrency: 2 },
  );
});
