import type { Logger } from "@micthiesen/mitools/logging";
import { getCarrierNamePatterns } from "../carriers/carrierMap.js";

const BLACKLISTED_SENDERS = [
  // Food delivery
  "@amazon.",
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

export interface EmailCandidate {
  from: string;
  subject: string;
  textBody: string;
}

export type FilterResult =
  | { pass: true; reason: string }
  | { pass: false; reason: string };

export async function filterTrackingCandidate(
  email: EmailCandidate,
  logger: Logger,
): Promise<FilterResult> {
  const fromLower = email.from.toLowerCase();

  // Blacklisted senders are always rejected
  if (BLACKLISTED_SENDERS.some((sender) => fromLower.includes(sender))) {
    return { pass: false, reason: "blacklisted sender" };
  }

  // Tier 1: Known carrier/shipping sender domains auto-pass
  if (CARRIER_SENDER_DOMAINS.some((domain) => fromLower.includes(domain))) {
    return { pass: true, reason: "carrier sender" };
  }

  // Tier 2: Keyword match in subject or body
  const searchText = `${email.subject} ${email.textBody}`.toLowerCase();
  const matchedKeyword = TRACKING_KEYWORDS.find((kw) => searchText.includes(kw));
  if (matchedKeyword) {
    return { pass: true, reason: `keyword "${matchedKeyword}"` };
  }

  // Tier 3: Carrier name mentioned (word-boundary match)
  const patterns = await getCarrierNamePatterns(logger);
  const fullText = `${email.subject} ${email.textBody}`;
  if (patterns.some((pattern) => pattern.test(fullText))) {
    return { pass: true, reason: "carrier name match" };
  }

  return { pass: false, reason: "no keyword match" };
}
