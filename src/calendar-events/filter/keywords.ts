import type { Logger } from "@micthiesen/mitools/logging";

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

export function isCalendarCandidate(email: EmailCandidate, logger: Logger): boolean {
  const fromLower = email.from.toLowerCase();

  // Blacklisted senders are always rejected
  if (BLACKLISTED_SENDERS.some((sender) => fromLower.includes(sender))) {
    logger.debug(`Calendar filter: blacklisted sender ${email.from}`);
    return false;
  }

  // Tier 1: Known booking/travel/event sender domains auto-pass
  if (AUTO_PASS_SENDERS.some((domain) => fromLower.includes(domain))) {
    logger.debug(`Calendar filter: auto-pass sender ${email.from}`);
    return true;
  }

  // Tier 2: Keyword match in subject or body
  const searchText = `${email.subject} ${email.textBody}`.toLowerCase();
  if (CALENDAR_KEYWORDS.some((keyword) => searchText.includes(keyword))) {
    return true;
  }

  return false;
}
