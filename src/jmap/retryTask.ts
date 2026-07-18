import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import type { JmapContext } from "./client.js";
import type { EmailHandler } from "./dispatcher.js";
import { fetchEmailById } from "./emailFetcher.js";
import {
  clearEmailRetry,
  EmailRetryEntity,
  enqueueEmailRetry,
  MAX_RETRY_ATTEMPTS,
  selectDueRetries,
} from "./retry.js";

/**
 * Replays transiently-failed email processing: pipelines enqueue retries via
 * enqueueEmailRetry, this task re-fetches each email and reruns the owning
 * pipeline's handler (pipeline dedup gates make that idempotent).
 */
/** Live JMAP handles, filled in once the pipelines connect (possibly late). */
export interface EmailPipelineControls {
  ctx?: JmapContext;
  handlers?: Map<string, EmailHandler>;
}

export default class EmailRetryTask extends ScheduledTask {
  public readonly name = "EmailRetry";
  public readonly schedule = "0 */15 * * * *"; // Every 15 minutes

  private readonly getControls: () => EmailPipelineControls;
  private readonly logger: Logger;
  private lastRunSummary: string | undefined;

  // The task is registered before the JMAP connection is (re)established so it
  // survives a failed connect at boot; it no-ops until controls are filled.
  constructor(getControls: () => EmailPipelineControls, logger: Logger) {
    super();
    this.getControls = getControls;
    this.logger = logger.extend("EmailRetry");
  }

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }

  public async run(): Promise<void> {
    const due = selectDueRetries(EmailRetryEntity.getAll());
    if (due.length === 0) {
      this.logger.debug("No email retries due");
      this.lastRunSummary = "No retries due";
      return;
    }

    const { ctx, handlers } = this.getControls();
    if (!ctx || !handlers) {
      this.lastRunSummary = `${due.length} due, pipelines not connected yet`;
      this.logger.info(
        `${due.length} retry(ies) due but the JMAP pipelines are not connected; deferring`,
      );
      return;
    }

    let succeeded = 0;
    let requeued = 0;
    let exhausted = 0;
    let missing = 0;
    let orphaned = 0;

    for (const row of due) {
      const handler = handlers.get(row.pipeline);
      if (!handler) {
        this.logger.warn(
          `No handler registered for pipeline "${row.pipeline}"; ` +
            `dropping retry for email ${row.emailId}`,
        );
        clearEmailRetry(row.pipeline, row.emailId);
        orphaned++;
        continue;
      }

      const email = await fetchEmailById(ctx, row.emailId, this.logger);
      if (!email) {
        this.logger.info(
          `Email ${row.emailId} no longer exists; dropping ${row.pipeline} retry`,
        );
        clearEmailRetry(row.pipeline, row.emailId);
        missing++;
        continue;
      }

      try {
        await handler.handleEmails([email]);
        // Pipelines swallow transient failures and re-enqueue instead of
        // throwing, so a resolved handler is NOT proof of success: only clear
        // the row if the run didn't just bump it again.
        const after = EmailRetryEntity.get({ retryKey: row.retryKey });
        if (after && after.attempts > row.attempts) {
          if (after.attempts > MAX_RETRY_ATTEMPTS) {
            clearEmailRetry(row.pipeline, row.emailId);
            exhausted++;
            this.logger.warn(
              `Giving up on ${row.pipeline} email "${email.subject}" after ` +
                `${row.attempts} attempts: ${after.reason}`,
            );
          } else {
            requeued++;
            this.logger.info(
              `Retry failed again for ${row.pipeline} email "${email.subject}" ` +
                `(attempt ${after.attempts}/${MAX_RETRY_ATTEMPTS}): ${after.reason}`,
            );
          }
          continue;
        }
        clearEmailRetry(row.pipeline, row.emailId);
        succeeded++;
        this.logger.info(
          `Retry succeeded for ${row.pipeline} email "${email.subject}" ` +
            `(attempt ${row.attempts})`,
        );
      } catch (error) {
        const reason = (error as Error).message ?? String(error);
        const attempts = row.attempts + 1;
        if (attempts > MAX_RETRY_ATTEMPTS) {
          clearEmailRetry(row.pipeline, row.emailId);
          exhausted++;
          this.logger.warn(
            `Giving up on ${row.pipeline} email "${email.subject}" after ` +
              `${row.attempts} attempts: ${reason}`,
          );
        } else {
          enqueueEmailRetry({
            pipeline: row.pipeline,
            emailId: row.emailId,
            reason,
          });
          requeued++;
          this.logger.info(
            `Retry failed for ${row.pipeline} email "${email.subject}" ` +
              `(attempt ${attempts}/${MAX_RETRY_ATTEMPTS}): ${reason}`,
          );
        }
      }
    }

    const orphanedSuffix = orphaned > 0 ? `, ${orphaned} orphaned` : "";
    this.lastRunSummary =
      `${due.length} due, ${succeeded} succeeded, ${requeued} requeued, ` +
      `${exhausted} exhausted, ${missing} missing${orphanedSuffix}`;
    this.logger.info(`Email retry pass: ${this.lastRunSummary}`);
  }
}
