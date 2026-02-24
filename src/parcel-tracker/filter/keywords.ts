const CARRIER_SENDER_DOMAINS = [
  "@amazon.",
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
  "shipped",
  "out for delivery",
  "tracking number",
  "order shipped",
  "in transit",
  "shipment",
  "estimated delivery",
  "delivery confirmation",
  "package",
];

export interface EmailCandidate {
  from: string;
  subject: string;
  textBody: string;
}

export function isTrackingCandidate(email: EmailCandidate): boolean {
  const fromLower = email.from.toLowerCase();

  // Tier 1: Known carrier/shipping sender domains auto-pass
  if (CARRIER_SENDER_DOMAINS.some((domain) => fromLower.includes(domain))) {
    return true;
  }

  // Tier 2: Keyword match in subject or body
  const searchText = `${email.subject} ${email.textBody}`.toLowerCase();
  return TRACKING_KEYWORDS.some((keyword) => searchText.includes(keyword));
}
