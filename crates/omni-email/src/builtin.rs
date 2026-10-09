//! Built-in sender lists of the parcel and calendar filters, shown read-only by
//! the rules UI and MCP tools and consulted when a user block rule would be
//! redundant. They live here (not in the pipeline crates) because the routes
//! and tools of this crate serve them; `omni-parcel` and `omni-calendar` use
//! these constants as their filter lists.

/// Parcel: always rejected (`src/parcel-tracker/filter/keywords.ts`).
pub const PARCEL_BLACKLISTED_SENDERS: &[&str] = &[
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

/// Parcel: known carrier/shipping sender domains that auto-pass.
pub const PARCEL_CARRIER_SENDER_DOMAINS: &[&str] = &[
    "@ups.com",
    "@fedex.com",
    "@usps.com",
    "@dhl.com",
    "@shopify.com",
    "@shop.app",
    "@narvar.com",
    "@aftership.com",
];

/// Calendar: always rejected (`src/calendar-events/filter/keywords.ts`).
pub const CALENDAR_BLACKLISTED_SENDERS: &[&str] = &[
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

/// Calendar: senders that auto-pass.
pub const CALENDAR_AUTO_PASS_SENDERS: &[&str] = &[
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
