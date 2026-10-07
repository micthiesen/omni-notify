import { Effect, Schema } from "effect";
import {
  readResetSource,
  resetIdentifier,
  resetText,
  resetTimestamp,
  resetUrl,
} from "../reset-alerts/source.js";

const labels = Schema.Array(resetIdentifier).pipe(Schema.check(Schema.isMaxLength(50)));
export const ClaudeResetSourceSchema = Schema.Struct({
  updated: resetTimestamp,
  events: Schema.Array(
    Schema.Struct({
      id: resetIdentifier,
      date: resetTimestamp,
      type: resetIdentifier,
      status: resetIdentifier,
      confidence: resetIdentifier,
      plans: labels,
      surfaces: labels,
      title: resetText,
      summary: resetText,
      sources: Schema.Array(Schema.Struct({ url: resetUrl })).pipe(
        Schema.check(Schema.isMaxLength(50)),
      ),
    }),
  ).pipe(Schema.check(Schema.isMaxLength(5_000))),
});

export type ClaudeResetSource = typeof ClaudeResetSourceSchema.Type;

export const readClaudeResetSource = Effect.fn("ClaudeResets.readSource")(function* () {
  return yield* readResetSource(
    "https://resetradar.com/data/events.json",
    ClaudeResetSourceSchema,
  );
});
