//! Which collection is the primary calendar.
//!
//! RFC 6764 discovery from the principal to the calendar home, then exactly
//! one VEVENT collection named `iCloud`. When the server reports a
//! `schedule-default-calendar-URL` it must be that collection. The choice is
//! pinned by path hash; a different but unambiguous answer re-pins
//! automatically, while zero or several candidates, or a default that names
//! another collection, fail closed.

use std::collections::BTreeSet;

use omni_core::digest::sha256_hex;
use omni_http::{HttpClient, Url};
use omni_store::Store;
use omni_store::entity::{EntityOps as _, EntityWrite as _, UpsertOpts};

use super::PrimaryError;
use super::client::{Multistatus, XmlNode, parse_multistatus};
use super::store::{PrimaryPin, SINGLETON};
use crate::caldav::http::{assert_trusted_caldav_url, basic_auth, propfind};
use crate::caldav::xml::{CalendarCollection, pick_calendar_collection};
use crate::caldav::{CaldavSettings, ICLOUD_CALDAV_ROOT};

const LOG: &str = "CalendarPrimary";

/// The display name of the primary calendar.
pub const PRIMARY_NAME: &str = "iCloud";

/// A resolved primary calendar. Never exposed in tool output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrimaryIdentity {
    pub collection_url: Url,
    pub path_sha256: String,
    pub auth_header: String,
    pub owner_addresses: BTreeSet<String>,
    pub is_server_default: Option<bool>,
    pub supports_sync: bool,
    pub writable: bool,
    pub pipeline_targets_primary: Option<bool>,
    pub resolved_at: i64,
}

fn multistatus(xml: &str, operation: &str) -> Result<Multistatus, PrimaryError> {
    parse_multistatus(xml).map_err(|e| PrimaryError::protocol(format!("{operation}: {e}")))
}

fn first_prop<'a>(ms: &'a Multistatus, name: &str) -> Option<&'a XmlNode> {
    ms.responses.iter().find_map(|r| r.prop(name))
}

fn resolve(base: &Url, href: &str) -> Result<Url, PrimaryError> {
    let joined = base
        .join(href)
        .map_err(|e| PrimaryError::protocol(format!("invalid href {href:?}: {e}")))?;
    assert_trusted_caldav_url(joined.as_str()).map_err(|e| PrimaryError::protocol(e.cause))
}

fn transport(error: crate::error::CaldavError) -> PrimaryError {
    PrimaryError::Transport {
        message: error.to_string(),
        transient: error.transient,
    }
}

/// One VEVENT-capable calendar collection in the home.
#[derive(Clone, Debug)]
struct Candidate {
    url: Url,
    name: String,
    supports_sync: bool,
    writable: bool,
    vevent: bool,
}

fn has_privilege(node: &XmlNode) -> bool {
    let mut privileges = Vec::new();
    node.find_all("privilege", &mut privileges);
    privileges.iter().any(|p| {
        p.children
            .iter()
            .any(|c| matches!(c.name.as_str(), "write-content" | "write" | "all"))
    })
}

/// Discovers and pins the primary calendar.
pub async fn resolve_primary(
    http: &HttpClient,
    settings: &CaldavSettings,
    store: &Store,
    now_ms: i64,
) -> Result<PrimaryIdentity, PrimaryError> {
    let auth = basic_auth(&settings.username, &settings.password);
    let root = Url::parse(ICLOUD_CALDAV_ROOT).map_err(|e| PrimaryError::protocol(e.to_string()))?;
    let principal = propfind(
        http,
        &root,
        &auth,
        "0",
        r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#,
    )
    .await
    .map_err(transport)?;
    let ms = multistatus(&principal.xml, "principal")?;
    let principal_href = first_prop(&ms, "current-user-principal")
        .and_then(XmlNode::href)
        .ok_or_else(|| PrimaryError::protocol("no current-user-principal"))?;
    let principal_url = resolve(&principal.url, &principal_href)?;

    let props = propfind(
        http,
        &principal_url,
        &auth,
        "0",
        r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><c:calendar-home-set/><c:calendar-user-address-set/><c:schedule-inbox-URL/></d:prop></d:propfind>"#,
    )
    .await
    .map_err(transport)?;
    let ms = multistatus(&props.xml, "principal properties")?;
    let home_href = first_prop(&ms, "calendar-home-set")
        .and_then(XmlNode::href)
        .ok_or_else(|| PrimaryError::protocol("no calendar-home-set"))?;
    let home_url = resolve(&props.url, &home_href)?;
    let mut owner_addresses = BTreeSet::new();
    if let Some(set) = first_prop(&ms, "calendar-user-address-set") {
        let mut hrefs = Vec::new();
        set.find_all("href", &mut hrefs);
        owner_addresses.extend(
            hrefs
                .iter()
                .filter_map(|h| super::model::mail_address(&h.text)),
        );
    }
    let inbox = first_prop(&ms, "schedule-inbox-url")
        .and_then(XmlNode::href)
        .and_then(|href| resolve(&props.url, &href).ok());

    let mut default_url: Option<Url> = None;
    if let Some(inbox) = inbox {
        match propfind(
            http,
            &inbox,
            &auth,
            "0",
            r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><c:schedule-default-calendar-URL/></d:prop></d:propfind>"#,
        )
        .await
        {
            Ok(response) => {
                default_url = multistatus(&response.xml, "schedule inbox")
                    .ok()
                    .as_ref()
                    .and_then(|ms| first_prop(ms, "schedule-default-calendar-url"))
                    .and_then(XmlNode::href)
                    .and_then(|href| resolve(&response.url, &href).ok());
            }
            Err(error) if error.transient => return Err(transport(error)),
            Err(error) => {
                tracing::debug!(target: LOG, "No schedule default calendar: {error}");
            }
        }
    }

    let listing = propfind(
        http,
        &home_url,
        &auth,
        "1",
        r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/"><d:prop><d:displayname/><d:resourcetype/><c:supported-calendar-component-set/><d:sync-token/><cs:getctag/><d:supported-report-set/><d:current-user-privilege-set/></d:prop></d:propfind>"#,
    )
    .await
    .map_err(transport)?;
    let ms = multistatus(&listing.xml, "calendar home")?;
    let mut collections: Vec<CalendarCollection> = Vec::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    for response in &ms.responses {
        let is_calendar = response
            .prop("resourcetype")
            .is_some_and(|rt| rt.child("calendar").is_some());
        if !is_calendar {
            continue;
        }
        let Ok(url) = resolve(&listing.url, &response.href) else {
            continue;
        };
        let name = response
            .prop("displayname")
            .map(XmlNode::trimmed)
            .unwrap_or_default();
        let components: Option<Vec<String>> = response
            .prop("supported-calendar-component-set")
            .map(|set| {
                set.children_named("comp")
                    .filter_map(|c| c.attr("name").map(str::to_ascii_uppercase))
                    .collect()
            });
        let vevent = components
            .as_ref()
            .is_none_or(|list| list.iter().any(|c| c == "VEVENT"));
        collections.push(CalendarCollection {
            href: response.href.clone(),
            name: name.clone(),
            components,
        });
        let supports_sync = response.prop("supported-report-set").is_some_and(|set| {
            let mut found = Vec::new();
            set.find_all("sync-collection", &mut found);
            !found.is_empty()
        });
        let writable = response
            .prop("current-user-privilege-set")
            .is_none_or(has_privilege);
        candidates.push(Candidate {
            url,
            name,
            supports_sync,
            writable,
            vevent,
        });
    }
    let named: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| c.vevent && c.name == PRIMARY_NAME)
        .collect();
    let chosen = match named.as_slice() {
        [only] => *only,
        [] => {
            return Err(PrimaryError::identity(
                "calendar_identity_ambiguous",
                format!("no calendar named \"{PRIMARY_NAME}\" accepts events"),
            ));
        }
        several => {
            return Err(PrimaryError::identity(
                "calendar_identity_ambiguous",
                format!(
                    "{} calendars are named \"{PRIMARY_NAME}\"; rename all but the primary one",
                    several.len()
                ),
            ));
        }
    };
    let is_server_default = match &default_url {
        Some(default)
            if default.path().trim_end_matches('/') != chosen.url.path().trim_end_matches('/') =>
        {
            return Err(PrimaryError::identity(
                "calendar_identity_mismatch",
                format!(
                    "the account's default calendar is not the calendar named \"{PRIMARY_NAME}\""
                ),
            ));
        }
        Some(_) => Some(true),
        None => None,
    };

    let path_sha256 = sha256_hex(chosen.url.path().as_bytes());
    let segment = chosen
        .url
        .path_segments()
        .and_then(|mut segments| segments.rfind(|s| !s.is_empty()))
        .unwrap_or_default()
        .to_owned();
    pin(store, &path_sha256, &segment, now_ms).await?;

    let pipeline_targets_primary = match &settings.calendar_url {
        Some(configured) => Url::parse(configured)
            .ok()
            .map(|u| u.path().trim_end_matches('/') == chosen.url.path().trim_end_matches('/')),
        None => pick_calendar_collection(&collections, settings.calendar_name.as_deref())
            .and_then(|c| resolve(&listing.url, &c.href).ok())
            .map(|u| u.path().trim_end_matches('/') == chosen.url.path().trim_end_matches('/')),
    };

    Ok(PrimaryIdentity {
        collection_url: chosen.url.clone(),
        path_sha256,
        auth_header: auth,
        owner_addresses,
        is_server_default,
        supports_sync: chosen.supports_sync,
        writable: chosen.writable,
        pipeline_targets_primary,
        resolved_at: now_ms,
    })
}

/// Writes the pin on first success; re-pins (with a warning) when an
/// unambiguous resolution names a different collection.
async fn pin(
    store: &Store,
    path_sha256: &str,
    segment: &str,
    now_ms: i64,
) -> Result<(), PrimaryError> {
    let path_sha256 = path_sha256.to_owned();
    let segment = segment.to_owned();
    store
        .write(move |tx| {
            let key = SINGLETON.to_owned();
            match tx.get::<PrimaryPin>(&key)? {
                Some(existing) if existing.path_sha256 == path_sha256 => Ok(()),
                Some(mut existing) => {
                    tracing::warn!(
                        target: LOG,
                        "Primary calendar changed (was \"{}\", now \"{segment}\"); re-pinned",
                        existing.collection_segment
                    );
                    existing.previous_path_sha256 = Some(existing.path_sha256.clone());
                    existing.path_sha256 = path_sha256;
                    existing.collection_segment = segment;
                    existing.repinned_at = Some(now_ms);
                    tx.upsert(&existing, UpsertOpts::default())
                }
                None => tx.upsert(
                    &PrimaryPin {
                        key,
                        collection_segment: segment,
                        path_sha256,
                        pinned_at: now_ms,
                        repinned_at: None,
                        previous_path_sha256: None,
                        extra: Default::default(),
                    },
                    UpsertOpts::default(),
                ),
            }
        })
        .await
        .map_err(PrimaryError::store)
}
