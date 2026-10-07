import { Effect, Schema } from "effect";
import {
  readResetSource,
  resetText as text,
  resetIdentifier as identifier,
  resetTimestamp as timestamp,
  resetUrl as url,
} from "../reset-alerts/source.js";
export { ResetSourceError } from "../reset-alerts/source.js";

const nullableText = Schema.NullOr(text);

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
      sourcePublishedAt: Schema.optional(Schema.NullOr(timestamp)),
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
      announcedAt: Schema.optional(timestamp),
      summary: Schema.optional(text),
      sources: Schema.Array(Schema.Struct({ announcementId: text, url })),
    }),
  ).pipe(Schema.check(Schema.isMaxLength(5_000))),
});

export type AlertFeed = typeof AlertFeedSchema.Type;
export type ResetHistory = typeof HistorySchema.Type;

export const readResetSources = Effect.fn("CodexResets.readSources")(function* () {
  return yield* Effect.all(
    {
      feed: readResetSource("https://resetbeacon.com/api/alerts", AlertFeedSchema),
      history: readResetSource("https://resetbeacon.com/api/history", HistorySchema),
    },
    { concurrency: 2 },
  );
});
