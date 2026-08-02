import type { Logger } from "@micthiesen/mitools/logging";
import { saveLastDispatchedAt } from "./persistence.js";
import type { EmailHandler, EmailTransport, FetchedEmail } from "./types.js";

/**
 * Transport-agnostic fan-out: on every mail event, polls the transport for
 * new emails and dispatches them to all registered handlers. Events that
 * land mid-processing set a pending flag and re-run (never dropped).
 */
export class EmailDispatcher {
  private transport: EmailTransport;
  private logger: Logger;
  private handlers: EmailHandler[] = [];
  private processing = false;
  private pending = false;

  constructor(transport: EmailTransport, logger: Logger) {
    this.transport = transport;
    this.logger = logger;
  }

  register(handler: EmailHandler): void {
    this.handlers.push(handler);
  }

  get handlerCount(): number {
    return this.handlers.length;
  }

  onMailEvent(): void {
    if (this.processing) {
      // Don't drop the signal: re-run once the current pass finishes so mail
      // events that land mid-processing are never lost.
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
          const { emails, commit } = await this.transport.pollNewEmails();
          if (emails.length > 0) {
            await this.dispatch(emails);
          }
          commit();
        } catch (error) {
          this.logger.error("Dispatcher error", (error as Error).message);
        }
      } while (this.pending);
    } finally {
      this.processing = false;
    }
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
}
