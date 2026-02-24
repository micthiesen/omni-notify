import type { Logger } from "@micthiesen/mitools/logging";
import { resolveCarrierCode } from "./carriers/carrierMap.js";
import { extractDeliveries } from "./extraction/extractDeliveries.js";
import { isTrackingCandidate } from "./filter/keywords.js";
import type { JmapContext } from "./jmap/client.js";
import { fetchNewEmails } from "./jmap/emailFetcher.js";
import { submitDelivery } from "./parcel/parcelApi.js";
import {
  getEmailState,
  hasSubmittedDelivery,
  recordSubmittedDelivery,
  saveEmailState,
} from "./persistence.js";

export class DeliveryPipeline {
  private logger: Logger;
  private ctx: JmapContext;
  private parcelApiKey: string;
  private processing = false;

  constructor(ctx: JmapContext, parcelApiKey: string, logger: Logger) {
    this.ctx = ctx;
    this.parcelApiKey = parcelApiKey;
    this.logger = logger;
  }

  async onEmailStateChange(): Promise<void> {
    if (this.processing) {
      this.logger.debug("Pipeline already processing, skipping");
      return;
    }

    this.processing = true;
    try {
      await this.processStateChange();
    } finally {
      this.processing = false;
    }
  }

  private async processStateChange(): Promise<void> {
    const sinceState = getEmailState();

    if (!sinceState) {
      // First run: save current state without processing
      this.logger.info("First run: fetching current JMAP state (skipping history)");
      const [result] = await this.ctx.jam.request([
        "Email/query",
        {
          accountId: this.ctx.accountId,
          filter: {},
          limit: 1,
        },
      ]);
      const queryState = (result as Record<string, unknown>).queryState as string;
      if (queryState) {
        saveEmailState(queryState);
        this.logger.info(`Saved initial JMAP state: ${queryState}`);
      }
      return;
    }

    let emails: Awaited<ReturnType<typeof fetchNewEmails>>["emails"];
    let newState: string;
    try {
      const result = await fetchNewEmails(this.ctx, sinceState, this.logger);
      emails = result.emails;
      newState = result.newState;
    } catch (error) {
      const message = (error as Error).message ?? "";
      if (message.includes("cannotCalculateChanges")) {
        this.logger.warn("cannotCalculateChanges: resetting state");
        // Fetch fresh state and skip this batch
        const [result] = await this.ctx.jam.request([
          "Email/query",
          {
            accountId: this.ctx.accountId,
            filter: {},
            limit: 1,
          },
        ]);
        const queryState = (result as Record<string, unknown>).queryState as string;
        if (queryState) saveEmailState(queryState);
        return;
      }
      this.logger.error(`Failed to fetch emails: ${message}`);
      return;
    }

    // Filter candidates
    const candidates = emails.filter((email) => {
      const isCandidate = isTrackingCandidate({
        from: email.from,
        subject: email.subject,
        textBody: email.textBody,
      });
      if (!isCandidate) {
        this.logger.debug(`Filtered out: "${email.subject}" from ${email.from}`);
      }
      return isCandidate;
    });

    if (candidates.length > 0) {
      this.logger.info(
        `${candidates.length} tracking candidate(s) from ${emails.length} new email(s)`,
      );
    } else if (emails.length > 0) {
      this.logger.debug(`No tracking candidates in ${emails.length} new email(s)`);
    }

    // Process each candidate
    for (const email of candidates) {
      try {
        await this.processEmail(email);
      } catch (error) {
        this.logger.error(
          `Failed to process email "${email.subject}": ${(error as Error).message}`,
        );
        // Continue with other emails
      }
    }

    // Save state after processing (crash-safe: dedup prevents double-submit)
    saveEmailState(newState);
  }

  private async processEmail(email: {
    id: string;
    subject: string;
    from: string;
    textBody: string;
  }): Promise<void> {
    this.logger.info(`Extracting from: "${email.subject}" (from: ${email.from})`);

    const deliveries = await extractDeliveries(
      { subject: email.subject, from: email.from, textBody: email.textBody },
      this.logger,
    );

    if (deliveries.length === 0) {
      this.logger.debug(`No tracking numbers found in "${email.subject}"`);
      return;
    }

    this.logger.info(`Found ${deliveries.length} delivery(ies) in "${email.subject}"`);

    for (const delivery of deliveries) {
      await this.processDelivery(delivery, email.id);
    }
  }

  private async processDelivery(
    delivery: { tracking_number: string; carrier: string; description: string },
    emailId: string,
  ): Promise<void> {
    // Dedup check
    if (hasSubmittedDelivery(delivery.tracking_number)) {
      this.logger.info(
        `Duplicate tracking number: ${delivery.tracking_number} (skipping)`,
      );
      return;
    }

    // Resolve carrier code
    const carrierResult = await resolveCarrierCode(delivery.carrier, this.logger);
    if (!carrierResult.resolved) {
      this.logger.warn(
        `Unknown carrier "${delivery.carrier}" for tracking ${delivery.tracking_number}. ` +
          "Add it to the carrier alias table.",
      );
      return;
    }

    // Skip Amazon — Parcel tracks those via account login
    if (carrierResult.carrierCode.startsWith("amzl")) {
      this.logger.info(
        `Amazon delivery (${carrierResult.carrierCode}), skipping: ${delivery.tracking_number}`,
      );
      return;
    }

    // Submit to Parcel
    const success = await submitDelivery(
      {
        trackingNumber: delivery.tracking_number,
        carrierCode: carrierResult.carrierCode,
        description: delivery.description,
      },
      this.parcelApiKey,
      this.logger,
    );

    if (success) {
      recordSubmittedDelivery({
        trackingNumber: delivery.tracking_number,
        carrierCode: carrierResult.carrierCode,
        description: delivery.description,
        submittedAt: Date.now(),
        emailId,
      });
    }
  }
}
