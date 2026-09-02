import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Data, Effect } from "effect";
import { runPromise } from "../effect/interop.js";
import {
  clearEmailRetry,
  claimEmailRetry,
  EmailRetryEntity,
  MAX_RETRY_ATTEMPTS,
  selectDueRetries,
} from "./retry.js";
import type { EmailHandler, EmailTransport } from "./types.js";

/**
 * Replays transiently-failed email processing: pipelines enqueue retries via
 * enqueueEmailRetry, this task re-fetches each email and reruns the owning
 * pipeline's handler (pipeline dedup gates make that idempotent).
 */
/** Live transport handles, filled in once the pipelines connect (possibly late). */
export interface EmailPipelineControls {
  transport?: EmailTransport;
  handlers?: Map<string, EmailHandler>;
}

class EmailRetryError extends Data.TaggedError("EmailRetryError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    return this.cause instanceof Error ? this.cause.message : String(this.cause);
  }
}

export default class EmailRetryTask extends ScheduledTask {
  public readonly name = "EmailRetry";
  public readonly schedule = "0 */15 * * * *"; // Every 15 minutes

  private readonly getControls: () => EmailPipelineControls;
  private readonly logger: Logger;
  private lastRunSummary: string | undefined;

  // The task is registered before the transport is (re)connected so it
  // survives a failed connect at boot; it no-ops until controls are filled.
  constructor(getControls: () => EmailPipelineControls, logger: Logger) {
    super();
    this.getControls = getControls;
    this.logger = logger.extend("EmailRetry");
  }

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }

  public run(): Promise<void> {
    return runPromise(this.runEffect);
  }

  private readonly runEffect = Effect.gen({ self: this }, function* () {
    const rows = EmailRetryEntity.getAll();
    const staleExhausted = rows.filter((row) => row.attempts >= MAX_RETRY_ATTEMPTS);
    for (const row of staleExhausted) {
      clearEmailRetry(row.pipeline, row.emailId);
    }
    const due = selectDueRetries(rows);
    if (due.length === 0) {
      this.logger.debug("No email retries due");
      this.lastRunSummary = "No retries due";
      return;
    }

    const { transport, handlers } = this.getControls();
    if (!transport || !handlers) {
      this.lastRunSummary = `${due.length} due, pipelines not connected yet`;
      this.logger.info(
        `${due.length} retry(ies) due but the email pipelines are not connected; deferring`,
      );
      return;
    }

    let succeeded = 0;
    let requeued = 0;
    let exhausted = 0;
    let missing = 0;
    let orphaned = 0;
    let permanent = 0;

    yield* Effect.forEach(
      due,
      (row) =>
        Effect.gen({ self: this }, function* () {
          const claimed = claimEmailRetry(row);
          const handler = handlers.get(row.pipeline);
          if (!handler) {
            this.logger.warn(
              `No handler registered for pipeline "${row.pipeline}"; ` +
                `dropping retry for email ${row.emailId}`,
            );
            clearEmailRetry(row.pipeline, row.emailId);
            orphaned++;
            return;
          }

          const fetched = yield* transport.fetchEmailByIdEffect(row.emailId).pipe(
            Effect.mapError(
              (cause) => new EmailRetryError({ operation: "fetch email", cause }),
            ),
            Effect.result,
          );
          if (fetched._tag === "Failure") {
            if (claimed.attempts >= MAX_RETRY_ATTEMPTS) {
              clearEmailRetry(row.pipeline, row.emailId);
              exhausted++;
            } else {
              requeued++;
            }
            this.logger.warn(
              `Could not fetch ${row.pipeline} email ${row.emailId} ` +
                `(attempt ${claimed.attempts}/${MAX_RETRY_ATTEMPTS}): ${fetched.failure.message}`,
            );
            return;
          }
          const email = fetched.success;
          if (!email) {
            this.logger.info(
              `Email ${row.emailId} no longer exists; dropping ${row.pipeline} retry`,
            );
            clearEmailRetry(row.pipeline, row.emailId);
            missing++;
            return;
          }

          yield* handler.handleEmailsEffect([email]).pipe(
            Effect.mapError(
              (cause) => new EmailRetryError({ operation: handler.name, cause }),
            ),
            Effect.matchEffect({
              onSuccess: () =>
                Effect.sync(() => {
                  // Pipelines swallow transient failures and re-enqueue instead of
                  // throwing, so a resolved handler is NOT proof of success: only clear
                  // the row if the run didn't just bump it again.
                  const after = EmailRetryEntity.get({ retryKey: row.retryKey });
                  if (after && (after.enqueueCount ?? 0) > (row.enqueueCount ?? 0)) {
                    if (claimed.attempts >= MAX_RETRY_ATTEMPTS) {
                      clearEmailRetry(row.pipeline, row.emailId);
                      exhausted++;
                      this.logger.warn(
                        `Giving up on ${row.pipeline} email "${email.subject}" after ` +
                          `${claimed.attempts} attempts: ${after.reason}`,
                      );
                    } else {
                      requeued++;
                      this.logger.info(
                        `Retry failed again for ${row.pipeline} email "${email.subject}" ` +
                          `(attempt ${claimed.attempts}/${MAX_RETRY_ATTEMPTS}): ${after.reason}`,
                      );
                    }
                    return;
                  }
                  clearEmailRetry(row.pipeline, row.emailId);
                  succeeded++;
                  this.logger.info(
                    `Retry succeeded for ${row.pipeline} email "${email.subject}" ` +
                      `(attempt ${claimed.attempts})`,
                  );
                }),
              onFailure: (error) =>
                Effect.sync(() => {
                  clearEmailRetry(row.pipeline, row.emailId);
                  permanent++;
                  this.logger.warn(
                    `Dropping permanent ${row.pipeline} retry for email ` +
                      `"${email.subject}": ${error.message}`,
                  );
                }),
            }),
          );
        }),
      { concurrency: 1, discard: true },
    );

    const orphanedSuffix = orphaned > 0 ? `, ${orphaned} orphaned` : "";
    const permanentSuffix = permanent > 0 ? `, ${permanent} permanent` : "";
    this.lastRunSummary =
      `${due.length} due, ${succeeded} succeeded, ${requeued} requeued, ` +
      `${exhausted + staleExhausted.length} exhausted, ${missing} missing` +
      `${orphanedSuffix}${permanentSuffix}`;
    this.logger.info(`Email retry pass: ${this.lastRunSummary}`);
  });
}
