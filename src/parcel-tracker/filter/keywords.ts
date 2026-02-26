import type { Logger } from "@micthiesen/mitools/logging";
import { getCarrierNamePatterns } from "../carriers/carrierMap.js";

const BLACKLISTED_SENDERS = [
  "@amazon.",
  "@uber.com",
  "@doordash.com",
  "@skipthedishes.com",
  "@instacart.com",
  "@fantuan.ca",
  "@ritual.co",
  "@toogoodtogo.com",
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

export interface EmailCandidate {
  from: string;
  subject: string;
  textBody: string;
}

export async function isTrackingCandidate(
  email: EmailCandidate,
  logger: Logger,
): Promise<boolean> {
  const fromLower = email.from.toLowerCase();

  // Blacklisted senders are always rejected
  if (BLACKLISTED_SENDERS.some((sender) => fromLower.includes(sender))) {
    return false;
  }

  // Tier 1: Known carrier/shipping sender domains auto-pass
  if (CARRIER_SENDER_DOMAINS.some((domain) => fromLower.includes(domain))) {
    return true;
  }

  // Tier 2: Keyword match in subject or body
  const searchText = `${email.subject} ${email.textBody}`.toLowerCase();
  if (TRACKING_KEYWORDS.some((keyword) => searchText.includes(keyword))) {
    return true;
  }

  // Tier 3: Carrier name mentioned (word-boundary match)
  const patterns = await getCarrierNamePatterns(logger);
  const fullText = `${email.subject} ${email.textBody}`;
  return patterns.some((pattern) => pattern.test(fullText));
}
