import type { Logger } from "@micthiesen/mitools/logging";
import type { JmapContext } from "./client.js";
import type { FetchedEmail } from "./emailFetcher.js";
import { fetchEmailsReceivedSince, fetchNewEmails } from "./emailFetcher.js";
import {
  getEmailState,
  getLastDispatchedAt,
  saveEmailState,
  saveLastDispatchedAt,
} from "./persistence.js";

/** Overlap window when recovering from a JMAP state reset: re-query emails
 * received up to this long before the last dispatch (pipelines dedup). */
const RECOVERY_OVERLAP_MS = 60 * 60_000;

export interface EmailHandler {
  name: string;
  handleEmails(emails: FetchedEmail[]): Promise<void>;
}

export class EmailDispatcher {
  private ctx: JmapContext;
  private logger: Logger;
  private handlers: EmailHandler[] = [];
  private processing = false;
  private pending = false;

  constructor(ctx: JmapContext, logger: Logger) {
    this.ctx = ctx;
    this.logger = logger;
  }

  register(handler: EmailHandler): void {
    this.handlers.push(handler);
  }

  get handlerCount(): number {
    return this.handlers.length;
  }

  onStateChange(): void {
    if (this.processing) {
      // Don't drop the signal: re-run once the current pass finishes so state
      // changes that land mid-processing are never lost.
      this.pending = true;
      this.logger.debug("Dispatcher already processing, queueing another pass");
      return;
    }

    this.processing = true;
    void this.processLoop();
  }

  private async processLoop(): Promise<void> {
    try {
      do {
        this.pending = false;
        try {
          await this.processStateChange();
        } catch (error) {
          this.logger.error("Dispatcher error", (error as Error).message);
        }
      } while (this.pending);
    } finally {
      this.processing = false;
    }
  }

  private async processStateChange(): Promise<void> {
    const sinceState = getEmailState();

    if (!sinceState) {
      this.logger.info("First run: fetching current JMAP state (skipping history)");
      const state = await this.fetchCurrentEmailState();
      if (state) {
        saveEmailState(state);
        this.logger.info(`Saved initial JMAP state: ${state}`);
      }
      return;
    }

    let emails: FetchedEmail[];
    let newState: string;
    try {
      const result = await fetchNewEmails(this.ctx, sinceState, this.logger);
      emails = result.emails;
      newState = result.newState;
    } catch (error) {
      const message = (error as Error).message ?? "";
      if (message.includes("cannotCalculateChanges")) {
        await this.recoverFromStateReset();
        return;
      }
      this.logger.error("Failed to fetch emails", message);
      return;
    }

    if (emails.length > 0) {
      await this.dispatch(emails);
    }

    saveEmailState(newState);
  }

  /**
   * The server can no longer diff from our saved state. Instead of silently
   * resetting (which drops everything received in the gap), run a bounded
   * Email/query for mail received after the last dispatch (minus an overlap),
   * dispatch it, then save the fresh state.
   */
  private async recoverFromStateReset(): Promise<void> {
    const lastDispatchedAt = getLastDispatchedAt();

    if (lastDispatchedAt === undefined) {
      this.logger.warn(
        "cannotCalculateChanges: JMAP state was reset with no last-dispatch " +
          "timestamp to recover from; resetting state only",
      );
      const state = await this.fetchCurrentEmailState();
      if (state) saveEmailState(state);
      return;
    }

    const sinceMs = lastDispatchedAt - RECOVERY_OVERLAP_MS;
    const { emails, state } = await fetchEmailsReceivedSince(
      this.ctx,
      sinceMs,
      this.logger,
    );
    if (emails.length > 0) {
      await this.dispatch(emails);
    }
    saveEmailState(state);
    this.logger.warn(
      `cannotCalculateChanges: JMAP state was reset; recovered ${emails.length} ` +
        `email(s) received since ${new Date(sinceMs).toISOString()}`,
    );
  }

  private async dispatch(emails: FetchedEmail[]): Promise<void> {
    const results = await Promise.allSettled(
      this.handlers.map((handler) => handler.handleEmails(emails)),
    );

    for (let i = 0; i < results.length; i++) {
      const result = results[i];
      if (result.status === "rejected") {
        this.logger.error(
          `Handler "${this.handlers[i].name}" failed`,
          (result.reason as Error).message,
        );
      }
    }

    saveLastDispatchedAt();
  }

  private async fetchCurrentEmailState(): Promise<string | undefined> {
    const [result] = await this.ctx.jam.request([
      "Email/get",
      { accountId: this.ctx.accountId, ids: [] },
    ]);
    return (result as Record<string, unknown>).state as string | undefined;
  }
}
