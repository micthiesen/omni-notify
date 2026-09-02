import { Entity } from "@micthiesen/mitools/entities";
import { Clock, Effect, Schema } from "effect";
import { fromSync } from "../effect/interop.js";

/**
 * Persisted retry queue for transiently-failed email processing (network/5xx
 * on Parcel submission or CalDAV writes). A retry re-fetches the email by id
 * and reruns the owning pipeline's handler; the pipelines' dedup gates make
 * that idempotent for anything that already landed.
 */
export type EmailRetryData = {
  /** `${pipeline}#${emailId}` */
  retryKey: string;
  pipeline: string;
  emailId: string;
  reason: string;
  /** Counts enqueue signals without consuming retry attempts. */
  enqueueCount?: number;
  attempts: number;
  nextAttemptAt: number;
  createdAt: number;
};

export const EmailRetryEntity = new Entity<EmailRetryData, ["retryKey"]>(
  "email-retry",
  ["retryKey"],
);

const EmailRetrySchema = Schema.Struct({
  retryKey: Schema.String,
  pipeline: Schema.String,
  emailId: Schema.String,
  reason: Schema.String,
  enqueueCount: Schema.optional(Schema.Number),
  attempts: Schema.Number,
  nextAttemptAt: Schema.Number,
  createdAt: Schema.Number,
});

export const MAX_RETRY_ATTEMPTS = 5;
const BASE_DELAY_MS = 30 * 60_000; // 30min, doubling per attempt

export function planEmailRetryEnqueue(
  existing: EmailRetryData | undefined,
  entry: { pipeline: string; emailId: string; reason: string },
  now: number,
): EmailRetryData {
  const retryKey = `${entry.pipeline}#${entry.emailId}`;
  const attempts = existing?.attempts ?? 0;
  return {
    retryKey,
    pipeline: entry.pipeline,
    emailId: entry.emailId,
    reason: entry.reason,
    enqueueCount: (existing?.enqueueCount ?? 0) + 1,
    attempts,
    nextAttemptAt: existing?.nextAttemptAt ?? now + retryDelayMs(1),
    createdAt: existing?.createdAt ?? now,
  };
}

/** Mark one due row as attempted before doing any network or handler work. */
function claimEmailRetry(row: EmailRetryData, now: number): EmailRetryData {
  const claimed = {
    ...row,
    attempts: row.attempts + 1,
    nextAttemptAt: now + retryDelayMs(row.attempts + 1),
  };
  EmailRetryEntity.upsert(claimed);
  return claimed;
}

export function retryDelayMs(attempts: number): number {
  return BASE_DELAY_MS * 2 ** Math.max(0, attempts - 1);
}

/** Pure: rows due for a retry now, exhausted rows excluded. */
export function selectDueRetries(
  rows: EmailRetryData[],
  now: number,
): EmailRetryData[] {
  return rows
    .filter((r) => r.attempts < MAX_RETRY_ATTEMPTS && r.nextAttemptAt <= now)
    .sort((a, b) => a.nextAttemptAt - b.nextAttemptAt);
}

function clearEmailRetry(pipeline: string, emailId: string): void {
  EmailRetryEntity.delete({ retryKey: `${pipeline}#${emailId}` });
}

const decodeRetry = (row: unknown) =>
  fromSync("decode email retry", () => Schema.decodeUnknownSync(EmailRetrySchema)(row));

export const EmailRetryPersistence = {
  getAll: Effect.fn("EmailRetry.getAll")(function* () {
    const rows = yield* fromSync("read email retries", () => EmailRetryEntity.getAll());
    return yield* Effect.forEach(rows, decodeRetry);
  }),
  get: Effect.fn("EmailRetry.get")(function* (retryKey: string) {
    const row = yield* fromSync("read email retry", () =>
      EmailRetryEntity.get({ retryKey }),
    );
    return row ? yield* decodeRetry(row) : undefined;
  }),
  enqueue: Effect.fn("EmailRetry.enqueue")(function* (entry: {
    pipeline: string;
    emailId: string;
    reason: string;
  }) {
    const now = yield* Clock.currentTimeMillis;
    yield* fromSync("enqueue email retry", () => {
      const retryKey = `${entry.pipeline}#${entry.emailId}`;
      const existing = EmailRetryEntity.get({ retryKey });
      EmailRetryEntity.upsert(planEmailRetryEnqueue(existing, entry, now));
    });
  }),
  claim: Effect.fn("EmailRetry.claim")(function* (row: EmailRetryData) {
    const now = yield* Clock.currentTimeMillis;
    return yield* fromSync("claim email retry", () => claimEmailRetry(row, now));
  }),
  clear: Effect.fn("EmailRetry.clear")(function* (pipeline: string, emailId: string) {
    yield* fromSync("clear email retry", () => clearEmailRetry(pipeline, emailId));
  }),
} as const;
