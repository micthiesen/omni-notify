//! Candidate filter: user block,
//! user allow, static blacklist, static auto-pass, then shared LLM triage, with
//! a keyword fallback only when triage is unavailable.

use omni_core::email::FetchedEmail;

use crate::support::{AdmitTier, EmailSupport, RuleVerdict, SupportError};

/// Built-in rejected senders (read-only; shown by the rules UI).
pub const BLACKLISTED_SENDERS: &[&str] = &[
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

/// Built-in auto-pass sender domains (read-only; shown by the rules UI).
pub const AUTO_PASS_SENDERS: &[&str] = &[
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

const CALENDAR_KEYWORDS: &[&str] = &[
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

/// The filter's decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterResult {
    Pass {
        reason: String,
        admit_tier: AdmitTier,
    },
    Reject {
        reason: String,
    },
}

impl FilterResult {
    pub fn passed(&self) -> bool {
        matches!(self, FilterResult::Pass { .. })
    }

    pub fn reason(&self) -> &str {
        match self {
            FilterResult::Pass { reason, .. } | FilterResult::Reject { reason } => reason,
        }
    }
}

/// Domain of a sender address; tolerates `Name <user@host>` by preferring the
/// bracketed address. `+tags` sit in the local part and drop out.
fn sender_domain(from_lower: &str) -> &str {
    let bracketed = from_lower.find('<').and_then(|open| {
        let rest = &from_lower[open + 1..];
        rest.find('>').map(|close| &rest[..close])
    });
    let addr = bracketed.unwrap_or(from_lower).trim();
    match addr.rfind('@') {
        Some(at) => addr[at + 1..].trim(),
        None => addr,
    }
}

fn is_blacklisted_sender(from_lower: &str, self_address: Option<&str>) -> bool {
    if BLACKLISTED_SENDERS.iter().any(|s| from_lower.contains(s)) {
        return true;
    }
    // The user's own outgoing mail is never a booking notification.
    self_address.is_some_and(|me| from_lower.contains(&me.to_lowercase()))
}

fn is_auto_pass(from_lower: &str) -> bool {
    let domain = sender_domain(from_lower);
    AUTO_PASS_SENDERS.iter().any(|entry| {
        let bare = entry.strip_prefix('@').unwrap_or(entry);
        domain == bare
            || domain
                .strip_suffix(bare)
                .is_some_and(|head| head.ends_with('.'))
    })
}

/// Degraded path when the triage model is unavailable.
fn keyword_fallback(email: &FetchedEmail) -> FilterResult {
    // Strip the ubiquitous footer first so it never masquerades as a signal.
    let search = format!("{} {}", email.subject, email.text_body)
        .to_lowercase()
        .replace("all rights reserved", "");
    match CALENDAR_KEYWORDS.iter().find(|kw| search.contains(*kw)) {
        Some(keyword) => FilterResult::Pass {
            reason: format!("keyword \"{keyword}\" (triage unavailable)"),
            admit_tier: AdmitTier::KeywordFallback,
        },
        None => FilterResult::Reject {
            reason: "no keyword match (triage unavailable)".to_owned(),
        },
    }
}

/// Decides whether an email is a calendar candidate. `self_address` is
/// `EMAIL_SELF_ADDRESS ?? ICLOUD_USERNAME`.
pub async fn filter_calendar_candidate(
    email: &FetchedEmail,
    support: &dyn EmailSupport,
    self_address: Option<&str>,
) -> Result<FilterResult, SupportError> {
    let from_lower = email.from.to_lowercase();

    // User rules beat the built-in lists in both directions.
    if let Some(rule) = support.find_sender_rule(&email.from).await? {
        return Ok(match rule.verdict {
            RuleVerdict::Block => FilterResult::Reject {
                reason: format!("blocked by rule {}", rule.pattern),
            },
            RuleVerdict::Allow => FilterResult::Pass {
                reason: format!("allowed by rule {}", rule.pattern),
                admit_tier: AdmitTier::Rule,
            },
        });
    }

    if is_blacklisted_sender(&from_lower, self_address) {
        return Ok(FilterResult::Reject {
            reason: "blacklisted sender".to_owned(),
        });
    }

    // Known booking/travel/event domains (and their subdomains) auto-pass.
    if is_auto_pass(&from_lower) {
        return Ok(FilterResult::Pass {
            reason: "known sender".to_owned(),
            admit_tier: AdmitTier::Builtin,
        });
    }

    // Cheap-LLM triage decides everything else; keywords are only the fallback.
    Ok(match support.classify(email).await {
        Ok(verdict) if verdict.calendar => FilterResult::Pass {
            reason: format!("triage: {}", verdict.reason),
            admit_tier: AdmitTier::Triage,
        },
        Ok(verdict) => FilterResult::Reject {
            reason: format!("triage: {}", verdict.reason),
        },
        Err(_) => keyword_fallback(email),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_domain_handles_display_names_and_tags() {
        assert_eq!(
            sender_domain("noreply@reminder.eventbrite.com"),
            "reminder.eventbrite.com"
        );
        assert_eq!(
            sender_domain("\"eventbrite\" <noreply+x@reminder.eventbrite.com>"),
            "reminder.eventbrite.com"
        );
        assert_eq!(sender_domain("no-at-sign"), "no-at-sign");
    }

    #[test]
    fn lookalike_domains_do_not_auto_pass() {
        assert!(is_auto_pass("noreply@eventbrite.com"));
        assert!(is_auto_pass("noreply@reminder.eventbrite.com"));
        assert!(!is_auto_pass("noreply@noteventbrite.com"));
    }
}
