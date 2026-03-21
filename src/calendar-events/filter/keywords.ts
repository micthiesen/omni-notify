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
  "@google.com",
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
  "reserved",
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
  "ticket",
  "your event",
  "concert",
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
  // General
  "calendar",
  "invite",
  "rsvp",
  "event on",
  "happening on",
];

export interface EmailCandidate {
  from: string;
  subject: string;
  textBody: string;
}

export type FilterResult =
  | { pass: true; reason: string }
  | { pass: false; reason: string };

export function filterCalendarCandidate(email: EmailCandidate): FilterResult {
  const fromLower = email.from.toLowerCase();

  // Blacklisted senders are always rejected
  if (BLACKLISTED_SENDERS.some((sender) => fromLower.includes(sender))) {
    return { pass: false, reason: "blacklisted sender" };
  }

  // Tier 1: Known booking/travel/event sender domains auto-pass
  if (AUTO_PASS_SENDERS.some((domain) => fromLower.includes(domain))) {
    return { pass: true, reason: "known sender" };
  }

  // Tier 2: Keyword match in subject or body
  const searchText = `${email.subject} ${email.textBody}`.toLowerCase();
  const matchedKeyword = CALENDAR_KEYWORDS.find((kw) => searchText.includes(kw));
  if (matchedKeyword) {
    return { pass: true, reason: `keyword "${matchedKeyword}"` };
  }

  return { pass: false, reason: "no keyword match" };
}
