import type { Logger } from "@micthiesen/mitools/logging";
import { generateText, Output } from "ai";
import { z } from "zod";
import { getTriageModel } from "../ai/registry.js";
import type { FetchedEmail } from "./emailFetcher.js";
import { formatFeedbackDigest } from "./feedback.js";

export interface TriageVerdict {
  parcel: boolean;
  calendar: boolean;
  reason: string;
}

/** What triage needs to see; `FetchedEmail` satisfies this shape. */
export type TriageEmail = Pick<FetchedEmail, "id" | "subject" | "from" | "textBody"> & {
  links?: string[];
};

const triageSchema = z.object({
  parcel: z.boolean(),
  calendar: z.boolean(),
  reason: z.string(),
});

const MAX_BODY_CHARS = 1500;
const MAX_LINKS = 5;
export const MAX_TRIAGE_CACHE_ENTRIES = 500;

export function buildTriagePrompt(email: TriageEmail): string {
  const body = email.textBody.slice(0, MAX_BODY_CHARS);
  const links = (email.links ?? []).slice(0, MAX_LINKS);
  const linksSection = links.length > 0 ? `\nLinks:\n${links.join("\n")}` : "";

  const digests = [
    ["Parcel pipeline", formatFeedbackDigest("parcel")],
    ["Calendar pipeline", formatFeedbackDigest("calendar")],
  ].filter(([, digest]) => digest !== "");
  const correctionsSection =
    digests.length > 0
      ? `\n\n## Recent user corrections — follow these\n${digests
          .map(([label, digest]) => `${label}:\n${digest}`)
          .join("\n")}`
      : "";

  return `Classify this email for two automated pipelines. Answer with a boolean per pipeline and a one-sentence reason covering both.

parcel — true only if the email plausibly carries or references a shipment tracking number for a physical package being delivered to the user (shipping confirmations, carrier updates, "your package is on the way"). Order confirmations WITHOUT tracking info, marketing, promotional digests, and order-status-only updates (payment received, order confirmed, awaiting confirmation) are false.

calendar — true only if the email describes a concrete upcoming appointment, booking, event, or service window worth putting on a personal calendar (reservations, flights, medical appointments, building maintenance notices). Newsletters, receipts for completed services, subscription/billing renewals, platform policy notices, and marketing "deadlines" are false. Genuine cancellations or reschedules of upcoming events are true.

From: ${email.from}
Subject: ${email.subject}

${body}${linksSection}${correctionsSection}`;
}

/**
 * One cheap-model relevance call per email, shared by both pipelines. Results
 * are memoized by email id (including the in-flight promise, so concurrent
 * pipelines cause exactly one model call); failures are never cached — the
 * keyword fallbacks in the filters own the degraded path.
 */
export class EmailTriageService {
  private cache = new Map<string, Promise<TriageVerdict>>();
  private logger: Logger;
  private classifyFn: (email: TriageEmail) => Promise<TriageVerdict>;

  constructor(
    logger: Logger,
    classifyFn?: (email: TriageEmail) => Promise<TriageVerdict>,
  ) {
    this.logger = logger;
    this.classifyFn = classifyFn ?? ((email) => this.callModel(email));
  }

  public classify(email: TriageEmail): Promise<TriageVerdict> {
    const cached = this.cache.get(email.id);
    if (cached) return cached;

    const pending = this.classifyFn(email).catch((error: unknown) => {
      // A failed classification must not poison the cache
      this.cache.delete(email.id);
      this.logger.warn(
        `Triage failed for "${email.subject}"`,
        (error as Error).message,
      );
      throw error;
    });

    this.cache.set(email.id, pending);
    if (this.cache.size > MAX_TRIAGE_CACHE_ENTRIES) {
      const oldest = this.cache.keys().next().value;
      if (oldest !== undefined) this.cache.delete(oldest);
    }
    return pending;
  }

  private async callModel(email: TriageEmail): Promise<TriageVerdict> {
    const { model, modelId } = getTriageModel();
    const result = await generateText({
      model,
      output: Output.object({ schema: triageSchema }),
      prompt: buildTriagePrompt(email),
    });
    const verdict = result.output;
    if (!verdict) throw new Error("Triage model returned no output");
    this.logger.info(
      `Triage (${modelId}) "${email.subject}": parcel=${verdict.parcel} ` +
        `calendar=${verdict.calendar} — ${verdict.reason}`,
    );
    return verdict;
  }
}
