import type { Logger } from "@micthiesen/mitools/logging";
import { findSenderRule } from "../../jmap/senderRules.js";
import type { EmailTriageService } from "../../jmap/triage.js";
import config from "../../utils/config.js";
import { getCarrierNamePatterns } from "../carriers/carrierMap.js";

const BLACKLISTED_SENDERS = [
  // Intentionally excluded: Parcel has a dedicated Amazon integration that
  // covers those deliveries, so tracking them here would duplicate.
  "@amazon.",
  // Food delivery
  "@uber.com",
  "@doordash.com",
  "@skipthedishes.com",
  "@instacart.com",
  "@fantuan.ca",
  "@ritual.co",
  "@toogoodtogo.com",
  // Newsletters & content
  "@substack.com",
  "@medium.com",
  "@patreon.com",
  // Marketing & SaaS
  "@coderabbit.ai",
  "@vercel.com",
  "@cloudflare.com",
  "@squarespace.com",
  // Finance
  "@wealthsimple.com",
  // Cloud platforms
  "cloudplatform-noreply@google.com",
  // Developer platforms ("Successfully published ... package" is not a parcel)
  "@npmjs.com",
  // Social media
  "@facebook.com",
  "@twitter.com",
  "@x.com",
  "@linkedin.com",
  "@instagram.com",
  "@reddit.com",
  "noreply@github.com",
];

const CARRIER_SENDER_DOMAINS = [
  "@ups.com",
  "@fedex.com",
  "@usps.com",
  "@dhl.com",
  "@shopify.com",
  "@shop.app",
  "@narvar.com",
  "@aftership.com",
];

const TRACKING_KEYWORDS = [
  "tracking",
  "track",
  "shipped",
  "out for delivery",
  "tracking number",
  "order shipped",
  "in transit",
  "shipment",
  "estimated delivery",
  "delivery confirmation",
  "package",
  "delivered",
  "delivery",
];

/**
 * AliExpress order-status subject shapes. These carry no tracking info and were
 * the single biggest source of wasted extraction calls; the same sender's
 * "Package ... ready to ship" emails must still pass.
 */
const ALIEXPRESS_ORDER_STATUS_PHRASES = [
  "awaiting confirmation",
  "order shipped",
  "order confirmed",
  "delivery update",
  "awaiting payment",
];

export function isAliexpressOrderStatus(fromLower: string, subject: string): boolean {
  if (!fromLower.includes("aliexpress")) return false;
  const subjectLower = subject.toLowerCase();
  if (/^order \d+:/.test(subjectLower)) return true;
  return ALIEXPRESS_ORDER_STATUS_PHRASES.some((p) => subjectLower.includes(p));
}

export interface EmailCandidate {
  id: string;
  from: string;
  subject: string;
  textBody: string;
  links?: string[];
}

export type FilterResult =
  | { pass: true; reason: string }
  | { pass: false; reason: string };

export async function filterTrackingCandidate(
  email: EmailCandidate,
  logger: Logger,
  triage: EmailTriageService,
): Promise<FilterResult> {
  const fromLower = email.from.toLowerCase();

  // User rule blocks beat everything
  const rule = findSenderRule(email.from, "parcel");
  if (rule?.verdict === "block") {
    return { pass: false, reason: `blocked by rule ${rule.pattern}` };
  }

  // Blacklisted senders are always rejected
  if (isBlacklistedSender(fromLower)) {
    return { pass: false, reason: "blacklisted sender" };
  }

  // AliExpress order-status emails never carry tracking info
  if (isAliexpressOrderStatus(fromLower, email.subject)) {
    return { pass: false, reason: "aliexpress order-status" };
  }

  // User rule allows skip triage entirely
  if (rule?.verdict === "allow") {
    return { pass: true, reason: `allowed by rule ${rule.pattern}` };
  }

  // Known carrier/shipping sender domains auto-pass
  if (CARRIER_SENDER_DOMAINS.some((domain) => fromLower.includes(domain))) {
    return { pass: true, reason: "carrier sender" };
  }

  // Cheap-LLM triage decides everything else; keywords are only the fallback
  try {
    const verdict = await triage.classify(email);
    return verdict.parcel
      ? { pass: true, reason: `triage: ${verdict.reason}` }
      : { pass: false, reason: `triage: ${verdict.reason}` };
  } catch {
    return keywordFallback(email, logger);
  }
}

function isBlacklistedSender(fromLower: string): boolean {
  if (BLACKLISTED_SENDERS.some((sender) => fromLower.includes(sender))) return true;
  // The user's own outgoing mail is never a shipment notification
  const self = config.FASTMAIL_USERNAME?.toLowerCase();
  return self !== undefined && fromLower.includes(self);
}

/** Degraded path when the triage model is unavailable. */
async function keywordFallback(
  email: EmailCandidate,
  logger: Logger,
): Promise<FilterResult> {
  const searchText = `${email.subject} ${email.textBody}`.toLowerCase();
  const matchedKeyword = TRACKING_KEYWORDS.find((kw) => searchText.includes(kw));
  if (matchedKeyword) {
    return { pass: true, reason: `keyword "${matchedKeyword}" (triage unavailable)` };
  }

  // Carrier name mentioned (word-boundary match)
  const patterns = await getCarrierNamePatterns(logger);
  const fullText = `${email.subject} ${email.textBody}`;
  if (patterns.some((pattern) => pattern.test(fullText))) {
    return { pass: true, reason: "carrier name match (triage unavailable)" };
  }

  return { pass: false, reason: "no keyword match (triage unavailable)" };
}
