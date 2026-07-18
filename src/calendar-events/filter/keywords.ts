import { findSenderRule } from "../../jmap/senderRules.js";
import type { EmailTriageService } from "../../jmap/triage.js";
import config from "../../utils/config.js";

const BLACKLISTED_SENDERS = [
  "@facebook.com",
  "@twitter.com",
  "@x.com",
  "@linkedin.com",
  "@instagram.com",
  "@pinterest.com",
  "@reddit.com",
  "noreply@github.com",
  "@medium.com",
  "@substack.com",
  "@patreon.com",
  "newsletter@",
  "marketing@",
  "promo@",
  "promotions@",
  "digest@",
  "news@",
  "no-reply@accounts.",
  "noreply@accounts.",
  "security@",
  "verify@",
  "password@",
  "@doordash.com",
  "@ubereats.com",
  "@skipthedishes.com",
  "@instacart.com",
  // Developer platforms ("event on ..." inside URLs is not a calendar event)
  "@npmjs.com",
  // Purchase "confirmation" emails, never appointments
  "@steampowered.com",
  // Shipment notifications belong to the parcel pipeline
  "pkginfo@ups.com",
];

const AUTO_PASS_SENDERS = [
  // Airlines
  "@united.com",
  "@delta.com",
  "@aa.com",
  "@aircanada.com",
  "@westjet.com",
  "@southwest.com",
  "@jetblue.com",
  "@alaskaair.com",
  "@spirit.com",
  "@porterairlines.com",
  "@flyflair.com",
  // Hotels
  "@marriott.com",
  "@hilton.com",
  "@ihg.com",
  "@hyatt.com",
  "@airbnb.com",
  "@vrbo.com",
  "@booking.com",
  "@hotels.com",
  "@expedia.com",
  "@fairmonthotels.com",
  // Events
  "@eventbrite.com",
  "@ticketmaster.com",
  "@stubhub.com",
  "@seatgeek.com",
  "@dice.fm",
  "@universe.com",
  // Medical
  "@zocdoc.com",
  "@healthgrades.com",
  // Ferries
  "@bcferries.com",
  // Restaurants
  "@opentable.com",
  "@resy.com",
  // Travel
  "@kayak.com",
  "@tripadvisor.com",
  // Building/strata management
  "@tribemgmt.com",
  // Scheduling
  "@calendly.com",
  "@acuityscheduling.com",
  "@squareup.com",
];

const CALENDAR_KEYWORDS = [
  // Booking
  "confirmation",
  "reservation",
  "booking",
  "booked",
  // Travel
  "itinerary",
  "flight",
  "boarding pass",
  "check-in",
  "check in",
  "hotel",
  "rental car",
  // Appointments
  "appointment",
  "scheduled for",
  "your visit",
  "reminder",
  // Events
  "your event",
  "show time",
  "game day",
  "admission",
  // Medical
  "your visit with",
  "dr.",
  "clinic",
  "dental",
  // Building/strata
  "shutdown",
  "maintenance",
  "strata",
  "building notice",
  "power outage",
  "water shutoff",
  // Cancellations/changes
  "cancelled",
  "canceled",
  "cancellation",
  "rescheduled",
  "reschedule",
  "schedule change",
  "time change",
  "date change",
  // General
  "calendar",
  "invite",
  "rsvp",
  "event on",
  "happening on",
];

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

/**
 * Domain portion of a sender address. In production `from` is a bare address
 * (`user@host`), but this also tolerates the display-name form `Name <user@host>` by
 * preferring the bracketed address. `+tags` live in the local part and are dropped with
 * everything before the last `@`.
 */
function senderDomain(fromLower: string): string {
  const bracketed = fromLower.match(/<([^>]*)>/);
  const addr = (bracketed ? bracketed[1] : fromLower).trim();
  const at = addr.lastIndexOf("@");
  return at >= 0 ? addr.slice(at + 1).trim() : addr;
}

export async function filterCalendarCandidate(
  email: EmailCandidate,
  triage: EmailTriageService,
): Promise<FilterResult> {
  const fromLower = email.from.toLowerCase();

  // User rule blocks beat everything
  const rule = findSenderRule(email.from, "calendar");
  if (rule?.verdict === "block") {
    return { pass: false, reason: `blocked by rule ${rule.pattern}` };
  }

  // Blacklisted senders are always rejected
  if (isBlacklistedSender(fromLower)) {
    return { pass: false, reason: "blacklisted sender" };
  }

  // User rule allows skip triage entirely
  if (rule?.verdict === "allow") {
    return { pass: true, reason: `allowed by rule ${rule.pattern}` };
  }

  // Known booking/travel/event domains auto-pass. Match on the sender's
  // domain (incl. subdomains) so transactional subdomains like
  // "noreply@reminder.eventbrite.com" still resolve to "eventbrite.com".
  const domain = senderDomain(fromLower);
  if (
    AUTO_PASS_SENDERS.some((entry) => {
      const bare = entry.replace(/^@/, "");
      return domain === bare || domain.endsWith(`.${bare}`);
    })
  ) {
    return { pass: true, reason: "known sender" };
  }

  // Cheap-LLM triage decides everything else; keywords are only the fallback
  try {
    const verdict = await triage.classify(email);
    return verdict.calendar
      ? { pass: true, reason: `triage: ${verdict.reason}` }
      : { pass: false, reason: `triage: ${verdict.reason}` };
  } catch {
    return keywordFallback(email);
  }
}

function isBlacklistedSender(fromLower: string): boolean {
  if (BLACKLISTED_SENDERS.some((sender) => fromLower.includes(sender))) return true;
  // The user's own outgoing mail is never a booking notification
  const self = config.FASTMAIL_USERNAME?.toLowerCase();
  return self !== undefined && fromLower.includes(self);
}

/** Degraded path when the triage model is unavailable. */
function keywordFallback(email: EmailCandidate): FilterResult {
  // Strip the ubiquitous "all rights reserved" footer first so it can never
  // masquerade as a booking signal.
  const searchText = `${email.subject} ${email.textBody}`
    .toLowerCase()
    .replaceAll("all rights reserved", "");
  const matchedKeyword = CALENDAR_KEYWORDS.find((kw) => searchText.includes(kw));
  if (matchedKeyword) {
    return { pass: true, reason: `keyword "${matchedKeyword}" (triage unavailable)` };
  }
  return { pass: false, reason: "no keyword match (triage unavailable)" };
}
