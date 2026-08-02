import type { Logger } from "@micthiesen/mitools/logging";
import { getLastDispatchedAt } from "../persistence.js";
import type {
  DownloadedAttachment,
  EmailAttachment,
  EmailPoll,
  EmailTransport,
  FetchedEmail,
} from "../types.js";
import { createJmapClient, type JmapContext } from "./client.js";
import {
  fetchEmailById,
  fetchEmailsReceivedSince,
  fetchNewEmails,
} from "./emailFetcher.js";
import { createEventSource } from "./eventSource.js";
import { getEmailState, saveEmailState } from "./persistence.js";

/** Overlap window when recovering from a JMAP state reset: re-query emails
 * received up to this long before the last dispatch (pipelines dedup). */
const RECOVERY_OVERLAP_MS = 60 * 60_000;

/**
 * Fastmail transport: JMAP Email/changes deltas + SSE EventSource push.
 * Stays alive until the iCloud MX cutover completes; selected via
 * EMAIL_TRANSPORT=fastmail (the default while FASTMAIL_API_TOKEN is set).
 */
export class JmapTransport implements EmailTransport {
  public readonly name = "JMAP";

  private ctx: JmapContext;
  private logger: Logger;
  private closeEventSource: (() => void) | undefined;

  private constructor(ctx: JmapContext, logger: Logger) {
    this.ctx = ctx;
    this.logger = logger;
  }

  static async create(bearerToken: string, logger: Logger): Promise<JmapTransport> {
    const ctx = await createJmapClient(bearerToken, logger);
    return new JmapTransport(ctx, logger);
  }

  async start(onMailEvent: () => void): Promise<void> {
    this.closeEventSource = await createEventSource(this.ctx, onMailEvent, this.logger);
  }

  stop(): void {
    this.closeEventSource?.();
  }

  async pollNewEmails(): Promise<EmailPoll> {
    const sinceState = getEmailState();

    if (!sinceState) {
      this.logger.info("First run: fetching current JMAP state (skipping history)");
      const state = await this.fetchCurrentEmailState();
      return {
        emails: [],
        commit: () => {
          if (state) {
            saveEmailState(state);
            this.logger.info(`Saved initial JMAP state: ${state}`);
          }
        },
      };
    }

    try {
      const { emails, newState } = await fetchNewEmails(
        this.ctx,
        sinceState,
        this.logger,
      );
      return { emails, commit: () => saveEmailState(newState) };
    } catch (error) {
      const message = (error as Error).message ?? "";
      if (message.includes("cannotCalculateChanges")) {
        return this.recoverFromStateReset();
      }
      throw error;
    }
  }

  /**
   * The server can no longer diff from our saved state. Instead of silently
   * resetting (which drops everything received in the gap), run a bounded
   * Email/query for mail received after the last dispatch (minus an overlap)
   * and resume from the fresh state.
   */
  private async recoverFromStateReset(): Promise<EmailPoll> {
    const lastDispatchedAt = getLastDispatchedAt();

    if (lastDispatchedAt === undefined) {
      this.logger.warn(
        "cannotCalculateChanges: JMAP state was reset with no last-dispatch " +
          "timestamp to recover from; resetting state only",
      );
      const state = await this.fetchCurrentEmailState();
      return {
        emails: [],
        commit: () => {
          if (state) saveEmailState(state);
        },
      };
    }

    const sinceMs = lastDispatchedAt - RECOVERY_OVERLAP_MS;
    const { emails, state } = await fetchEmailsReceivedSince(
      this.ctx,
      sinceMs,
      this.logger,
    );
    this.logger.warn(
      `cannotCalculateChanges: JMAP state was reset; recovered ${emails.length} ` +
        `email(s) received since ${new Date(sinceMs).toISOString()}`,
    );
    return { emails, commit: () => saveEmailState(state) };
  }

  fetchEmailById(emailId: string): Promise<FetchedEmail | undefined> {
    return fetchEmailById(this.ctx, emailId, this.logger);
  }

  async downloadAttachment(
    attachment: EmailAttachment,
  ): Promise<DownloadedAttachment | undefined> {
    try {
      const response = await this.ctx.jam.downloadBlob({
        accountId: this.ctx.accountId,
        blobId: attachment.blobId,
        mimeType: attachment.type,
        fileName: attachment.name,
      });

      if (!response.ok) {
        this.logger.warn(
          `Failed to download "${attachment.name}": ${response.status} ${response.statusText}`,
        );
        return undefined;
      }

      return {
        name: attachment.name,
        mimeType: attachment.type,
        data: Buffer.from(await response.arrayBuffer()),
      };
    } catch (error) {
      this.logger.warn(
        `Error downloading "${attachment.name}"`,
        (error as Error).message,
      );
      return undefined;
    }
  }

  private async fetchCurrentEmailState(): Promise<string | undefined> {
    const [result] = await this.ctx.jam.request([
      "Email/get",
      { accountId: this.ctx.accountId, ids: [] },
    ]);
    return (result as Record<string, unknown>).state as string | undefined;
  }
}
