import { Entity } from "@micthiesen/mitools/entities";

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
  attempts: number;
  nextAttemptAt: number;
  createdAt: number;
};

export const EmailRetryEntity = new Entity<EmailRetryData, ["retryKey"]>(
  "email-retry",
  ["retryKey"],
);

export const MAX_RETRY_ATTEMPTS = 5;
const BASE_DELAY_MS = 30 * 60_000; // 30min, doubling per attempt

export function enqueueEmailRetry(entry: {
  pipeline: string;
  emailId: string;
  reason: string;
}): void {
  const retryKey = `${entry.pipeline}#${entry.emailId}`;
  const existing = EmailRetryEntity.get({ retryKey });
  const attempts = (existing?.attempts ?? 0) + 1;
  EmailRetryEntity.upsert({
    retryKey,
    pipeline: entry.pipeline,
    emailId: entry.emailId,
    reason: entry.reason,
    attempts,
    nextAttemptAt: Date.now() + retryDelayMs(attempts),
    createdAt: existing?.createdAt ?? Date.now(),
  });
}

export function retryDelayMs(attempts: number): number {
  return BASE_DELAY_MS * 2 ** Math.max(0, attempts - 1);
}

/** Pure: rows due for a retry now, exhausted rows excluded. */
export function selectDueRetries(
  rows: EmailRetryData[],
  now = Date.now(),
): EmailRetryData[] {
  return rows
    .filter((r) => r.attempts <= MAX_RETRY_ATTEMPTS && r.nextAttemptAt <= now)
    .sort((a, b) => a.nextAttemptAt - b.nextAttemptAt);
}

export function clearEmailRetry(pipeline: string, emailId: string): void {
  EmailRetryEntity.delete({ retryKey: `${pipeline}#${emailId}` });
}
