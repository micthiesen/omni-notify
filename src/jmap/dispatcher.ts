import type { Logger } from "@micthiesen/mitools/logging";
import type { JmapContext } from "./client.js";
import type { FetchedEmail } from "./emailFetcher.js";
import { fetchNewEmails } from "./emailFetcher.js";
import { getEmailState, saveEmailState } from "./persistence.js";

export interface EmailHandler {
  name: string;
  handleEmails(emails: FetchedEmail[]): Promise<void>;
}

export class EmailDispatcher {
  private ctx: JmapContext;
  private logger: Logger;
  private handlers: EmailHandler[] = [];
  private processing = false;

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
      this.logger.debug("Dispatcher already processing, skipping");
      return;
    }

    this.processing = true;
    this.processStateChange()
      .catch((error) => {
        this.logger.error("Dispatcher error", (error as Error).message);
      })
      .finally(() => {
        this.processing = false;
      });
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
        this.logger.warn("cannotCalculateChanges: resetting state");
        const state = await this.fetchCurrentEmailState();
        if (state) saveEmailState(state);
        return;
      }
      this.logger.error("Failed to fetch emails", message);
      return;
    }

    if (emails.length > 0) {
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
    }

    saveEmailState(newState);
  }

  private async fetchCurrentEmailState(): Promise<string | undefined> {
    const [result] = await this.ctx.jam.request([
      "Email/get",
      { accountId: this.ctx.accountId, ids: [] },
    ]);
    return (result as Record<string, unknown>).state as string | undefined;
  }
}
