//! The local mirror of the primary calendar and its change feed.
//!
//! `sync-collection` (RFC 6578) reports changed and removed hrefs since the
//! stored token; an invalid token (or a server without the report) falls back
//! to a full ETag listing diffed against the mirror. Changed resources are
//! fetched with `calendar-multiget`. Mirror rows, change rows and the new
//! token commit in one transaction, so a crash before the commit re-detects
//! the same changes and a crash after it never loses them. The first sync of
//! a collection records a baseline and no changes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use jiff::tz::TimeZone;
use omni_http::Url;
use omni_store::Store;
use omni_store::entity::{EntityOps as _, EntityWrite as _, UpsertOpts};
use regex::Regex;

use super::PrimaryError;
use super::client::{DavClient, ReadError, error_names, parse_multistatus, xml_escape};
use super::ics::IcsDoc;
use super::identity::PrimaryIdentity;
use super::model::{self, Projection};
use super::store::{
    CHANGE_MAX_ROWS, CHANGE_RETENTION_MS, ChangeKind, ChangeOrigin, ChangeRow, DELETED,
    MirrorResource, SINGLETON, SyncState, WriteEcho, change_key, echo_key,
};
use super::time::Zones;

const LOG: &str = "CalendarPrimary";

/// Rounds of truncated sync-collection results followed per sync.
const MAX_SYNC_ROUNDS: usize = 20;
/// Hrefs per calendar-multiget.
const MULTIGET_BATCH: usize = 50;

static EVENT_ID: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._@%+-]{0,199}\.ics$").ok());

/// Whether `event_id` is a valid resource name (no `/`, no `..`).
pub fn is_valid_event_id(event_id: &str) -> bool {
    !event_id.contains("..") && EVENT_ID.as_ref().is_some_and(|re| re.is_match(event_id))
}

/// The resource URL of an event in the primary collection.
pub fn event_url(collection: &Url, event_id: &str) -> Result<Url, PrimaryError> {
    if !is_valid_event_id(event_id) {
        return Err(PrimaryError::coded(
            "invalid_event_id",
            "eventId must be a resource name such as ABC-123.ics",
        ));
    }
    let url = collection
        .join(event_id)
        .map_err(|e| PrimaryError::protocol(e.to_string()))?;
    let parent = collection.path().trim_end_matches('/');
    let expected = format!("{parent}/{event_id}");
    if url.path() != expected {
        return Err(PrimaryError::coded(
            "invalid_event_id",
            "eventId does not resolve inside the calendar",
        ));
    }
    Ok(url)
}

/// The resource name of an href directly inside the collection.
pub fn event_id_from_href(collection: &Url, href: &str) -> Option<String> {
    let url = collection.join(href).ok()?;
    let parent = collection.path().trim_end_matches('/');
    let name = url.path().strip_prefix(parent)?.strip_prefix('/')?;
    (is_valid_event_id(name)).then(|| name.to_owned())
}

/// What a sync did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub full: bool,
    pub baseline: bool,
    pub fetched: usize,
    pub changes: usize,
    pub incomplete: bool,
}

pub struct SyncCtx<'a> {
    pub client: &'a DavClient,
    pub identity: &'a PrimaryIdentity,
    pub store: &'a Store,
    pub now_ms: i64,
    pub default_tz: &'a TimeZone,
}

/// One fetched resource.
#[derive(Clone, Debug)]
pub struct Fetched {
    pub event_id: String,
    pub etag: String,
    /// `None` when oversize.
    pub ics: Option<String>,
}

enum Listing {
    /// From sync-collection: changed (with ETags) and removed resources.
    Delta {
        changed: BTreeMap<String, String>,
        removed: BTreeSet<String>,
        token: Option<String>,
        incomplete: bool,
    },
    /// A full ETag listing.
    Full(BTreeMap<String, String>),
}

fn read_error(operation: &str, error: ReadError) -> PrimaryError {
    match error {
        ReadError::TooLarge(limit) => PrimaryError::protocol(format!(
            "{operation}: response exceeds the {limit}-byte limit"
        )),
        ReadError::Transport(message) => PrimaryError::Transport {
            message: format!("{operation}: {message}"),
            transient: true,
        },
    }
}

fn http_failure(operation: &str, status: u16, excerpt: &str) -> PrimaryError {
    PrimaryError::Transport {
        message: format!("{operation}: HTTP {status} {excerpt}"),
        transient: status >= 500 || status == 429,
    }
}

async fn sync_collection(
    ctx: &SyncCtx<'_>,
    token: Option<&str>,
) -> Result<Option<Listing>, PrimaryError> {
    let collection = &ctx.identity.collection_url;
    let mut token = token.map(str::to_owned);
    let mut changed = BTreeMap::new();
    let mut removed = BTreeSet::new();
    for _ in 0..MAX_SYNC_ROUNDS {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><d:sync-collection xmlns:d="DAV:"><d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"#,
            xml_escape(token.as_deref().unwrap_or_default())
        );
        let response = ctx
            .client
            .report(collection, "0", &body)
            .await
            .map_err(|e| read_error("sync-collection", e))?;
        if matches!(response.status, 403 | 409) && error_names(&response.text(), "valid-sync-token")
        {
            tracing::info!(target: LOG, "Sync token rejected; running a full resync");
            return Ok(None);
        }
        if response.status != 207 {
            return Err(http_failure(
                "sync-collection",
                response.status,
                &response.excerpt(),
            ));
        }
        let ms = parse_multistatus(&response.text())
            .map_err(|e| PrimaryError::protocol(format!("sync-collection: {e}")))?;
        let mut truncated = false;
        for item in &ms.responses {
            if item.status == Some(507) {
                truncated = true;
                continue;
            }
            let Some(event_id) = event_id_from_href(collection, &item.href) else {
                continue;
            };
            if item.status == Some(404) {
                changed.remove(&event_id);
                removed.insert(event_id);
                continue;
            }
            if let Some(etag) = item.prop_text("getetag") {
                removed.remove(&event_id);
                changed.insert(event_id, etag);
            }
        }
        let next = ms.sync_token.clone();
        if !truncated {
            return Ok(Some(Listing::Delta {
                changed,
                removed,
                token: next,
                incomplete: false,
            }));
        }
        if next.is_none() || next == token {
            break;
        }
        token = next;
    }
    Ok(Some(Listing::Delta {
        changed,
        removed,
        token: None,
        incomplete: true,
    }))
}

async fn full_listing(ctx: &SyncCtx<'_>) -> Result<Listing, PrimaryError> {
    let collection = &ctx.identity.collection_url;
    let response = ctx
        .client
        .propfind(
            collection,
            "1",
            r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:getetag/></d:prop></d:propfind>"#,
        )
        .await
        .map_err(|e| read_error("list calendar", e))?;
    if response.status != 207 {
        return Err(http_failure(
            "list calendar",
            response.status,
            &response.excerpt(),
        ));
    }
    let ms = parse_multistatus(&response.text())
        .map_err(|e| PrimaryError::protocol(format!("list calendar: {e}")))?;
    let mut etags = BTreeMap::new();
    for item in &ms.responses {
        if let (Some(event_id), Some(etag)) = (
            event_id_from_href(collection, &item.href),
            item.prop_text("getetag"),
        ) {
            etags.insert(event_id, etag);
        }
    }
    Ok(Listing::Full(etags))
}

/// Fetches resources with calendar-multiget, halving a batch whose response
/// is too large; a single oversize resource is returned without content.
/// Resources the server no longer has are omitted.
pub async fn multiget(
    client: &DavClient,
    collection: &Url,
    event_ids: &[String],
) -> Result<Vec<Fetched>, PrimaryError> {
    let mut out = Vec::new();
    let mut queue: Vec<Vec<String>> = event_ids
        .chunks(MULTIGET_BATCH)
        .map(<[String]>::to_vec)
        .collect();
    queue.reverse();
    while let Some(batch) = queue.pop() {
        let hrefs: String = batch
            .iter()
            .filter_map(|id| event_url(collection, id).ok())
            .map(|url| format!("<d:href>{}</d:href>", xml_escape(url.path())))
            .collect();
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop>{hrefs}</c:calendar-multiget>"#
        );
        let response = match client.report(collection, "1", &body).await {
            Ok(response) => response,
            Err(ReadError::TooLarge(_)) if batch.len() > 1 => {
                let (a, b) = batch.split_at(batch.len() / 2);
                queue.push(b.to_vec());
                queue.push(a.to_vec());
                continue;
            }
            Err(ReadError::TooLarge(_)) => {
                let fetched = single_get(client, collection, &batch[0]).await?;
                out.extend(fetched);
                continue;
            }
            Err(e) => return Err(read_error("calendar-multiget", e)),
        };
        if response.status != 207 {
            return Err(http_failure(
                "calendar-multiget",
                response.status,
                &response.excerpt(),
            ));
        }
        let ms = parse_multistatus(&response.text())
            .map_err(|e| PrimaryError::protocol(format!("calendar-multiget: {e}")))?;
        for item in &ms.responses {
            let Some(event_id) = event_id_from_href(collection, &item.href) else {
                continue;
            };
            if item.status.is_some_and(|s| s == 404) {
                continue;
            }
            let (Some(etag), Some(data)) = (item.prop_text("getetag"), item.calendar_data()) else {
                continue;
            };
            let oversize = data.len() > super::client::RESOURCE_MAX_BYTES;
            out.push(Fetched {
                event_id,
                etag,
                ics: (!oversize).then(|| data.to_owned()),
            });
        }
    }
    Ok(out)
}

/// One resource by GET (used when it alone exceeds the multiget cap).
async fn single_get(
    client: &DavClient,
    collection: &Url,
    event_id: &str,
) -> Result<Option<Fetched>, PrimaryError> {
    let url = event_url(collection, event_id)?;
    match client.get(&url).await {
        Ok(response) if response.status == 404 => Ok(None),
        Ok(response) if response.is_success() => Ok(Some(Fetched {
            event_id: event_id.to_owned(),
            etag: response.etag.clone().unwrap_or_default(),
            ics: Some(response.text()),
        })),
        Ok(response) => Err(http_failure(
            "get event",
            response.status,
            &response.excerpt(),
        )),
        Err(ReadError::TooLarge(_)) => Ok(Some(Fetched {
            event_id: event_id.to_owned(),
            etag: String::new(),
            ics: None,
        })),
        Err(e) => Err(read_error("get event", e)),
    }
}

/// The mirror row for a fetched resource.
pub fn mirror_row(
    fetched: &Fetched,
    identity: &PrimaryIdentity,
    default_tz: &TimeZone,
    now_ms: i64,
) -> MirrorResource {
    let parsed = fetched.ics.as_deref().and_then(|t| IcsDoc::parse(t).ok());
    let (uid, projection) = match &parsed {
        Some(doc) => {
            let zones = Zones::new(doc, default_tz.clone());
            (
                model::primary_event(doc).and_then(|e| e.uid()),
                model::projection(doc, &zones, &identity.owner_addresses),
            )
        }
        None => (None, Projection::default()),
    };
    MirrorResource {
        event_id: fetched.event_id.clone(),
        etag: fetched.etag.clone(),
        uid,
        ics: fetched.ics.clone(),
        projection,
        seen_at: now_ms,
        oversize: fetched.ics.is_none(),
        extra: Default::default(),
    }
}

/// Unfolded content lines without DTSTAMP, for churn suppression.
fn normalized_body(ics: &str) -> Vec<String> {
    let unfolded = ics
        .replace("\r\n ", "")
        .replace("\r\n\t", "")
        .replace("\n ", "")
        .replace("\n\t", "");
    unfolded
        .lines()
        .map(|l| l.trim_end_matches('\r').to_owned())
        .filter(|l| !l.is_empty() && !l.to_ascii_uppercase().starts_with("DTSTAMP"))
        .collect()
}

/// The change between two mirror rows, or `None` when nothing meaningful
/// changed (ETag churn).
fn describe_update(before: &MirrorResource, after: &MirrorResource) -> Option<Vec<String>> {
    match (before.ics.as_deref(), after.ics.as_deref()) {
        (Some(a), Some(b)) => {
            if normalized_body(a) == normalized_body(b) {
                return None;
            }
            match (IcsDoc::parse(a), IcsDoc::parse(b)) {
                (Ok(da), Ok(db)) => Some(model::changed_fields(&da, &db)),
                _ => Some(vec!["other".to_owned()]),
            }
        }
        _ => Some(vec!["other".to_owned()]),
    }
}

/// Runs one sync (callers serialize it).
pub async fn sync(ctx: &SyncCtx<'_>) -> Result<SyncReport, PrimaryError> {
    let state = ctx
        .store
        .read(|docs| docs.get::<SyncState>(&SINGLETON.to_owned()))
        .await
        .map_err(PrimaryError::store)?
        .unwrap_or_else(|| SyncState {
            key: SINGLETON.to_owned(),
            ..SyncState::default()
        });
    let same_collection =
        state.collection_sha256.as_deref() == Some(ctx.identity.path_sha256.as_str());
    let baseline = !state.baselined || !same_collection;
    let mirror: BTreeMap<String, MirrorResource> = if same_collection {
        ctx.store
            .read(|docs| docs.get_all::<MirrorResource>())
            .await
            .map_err(PrimaryError::store)?
            .into_iter()
            .map(|r| (r.event_id.clone(), r))
            .collect()
    } else {
        BTreeMap::new()
    };

    let token = if same_collection {
        state.sync_token.as_deref()
    } else {
        None
    };
    let listing = if ctx.identity.supports_sync {
        match sync_collection(ctx, token).await? {
            Some(listing) => listing,
            None => full_listing(ctx).await?,
        }
    } else {
        full_listing(ctx).await?
    };
    let mut report = SyncReport {
        baseline,
        ..SyncReport::default()
    };
    let (to_fetch, removed, new_token, full) = match listing {
        Listing::Delta {
            changed,
            removed,
            token,
            incomplete,
        } => {
            report.incomplete = incomplete;
            let to_fetch: Vec<String> = changed
                .into_iter()
                .filter(|(id, etag)| mirror.get(id).is_none_or(|m| &m.etag != etag))
                .map(|(id, _)| id)
                .collect();
            let removed: Vec<String> = removed
                .into_iter()
                .filter(|id| mirror.contains_key(id))
                .collect();
            (to_fetch, removed, token, false)
        }
        Listing::Full(etags) => {
            let to_fetch: Vec<String> = etags
                .iter()
                .filter(|(id, etag)| mirror.get(*id).is_none_or(|m| &m.etag != *etag))
                .map(|(id, _)| id.clone())
                .collect();
            let removed: Vec<String> = mirror
                .keys()
                .filter(|id| !etags.contains_key(*id))
                .cloned()
                .collect();
            (to_fetch, removed, None, true)
        }
    };
    report.full = full;
    let fetched = multiget(ctx.client, &ctx.identity.collection_url, &to_fetch).await?;
    report.fetched = fetched.len();
    let fetched_ids: BTreeSet<&str> = fetched.iter().map(|f| f.event_id.as_str()).collect();
    // A changed href the server no longer returns is gone.
    let mut removed = removed;
    for id in &to_fetch {
        if !fetched_ids.contains(id.as_str()) && mirror.contains_key(id) {
            removed.push(id.clone());
        }
    }

    let rows: Vec<MirrorResource> = fetched
        .iter()
        .map(|f| mirror_row(f, ctx.identity, ctx.default_tz, ctx.now_ms))
        .collect();
    let now = ctx.now_ms;
    let path_sha256 = ctx.identity.path_sha256.clone();
    let incomplete = report.incomplete;
    let changes = ctx
        .store
        .write(move |tx| {
            let mut state = tx
                .get::<SyncState>(&SINGLETON.to_owned())?
                .unwrap_or_else(|| SyncState {
                    key: SINGLETON.to_owned(),
                    ..SyncState::default()
                });
            if !same_collection {
                tx.delete_all::<MirrorResource>()?;
            }
            let mut pending: Vec<ChangeRow> = Vec::new();
            for row in &rows {
                let before = mirror.get(&row.event_id);
                if !baseline {
                    let (kind, fields) = match before {
                        None => (ChangeKind::Created, Some(Vec::new())),
                        Some(b) => (ChangeKind::Updated, describe_update(b, row)),
                    };
                    if let Some(fields) = fields {
                        let echoed = tx
                            .get::<WriteEcho>(&echo_key(&row.event_id, &row.etag))?
                            .is_some();
                        pending.push(ChangeRow {
                            key: String::new(),
                            seq: 0,
                            event_id: row.event_id.clone(),
                            uid: row.uid.clone(),
                            kind,
                            before: before.map(|b| b.projection.clone()),
                            after: Some(row.projection.clone()),
                            changed_fields: fields,
                            etag: Some(row.etag.clone()),
                            previous_etag: before.map(|b| b.etag.clone()),
                            origin: if echoed {
                                ChangeOrigin::Omni
                            } else {
                                ChangeOrigin::External
                            },
                            detected_at: now,
                            extra: Default::default(),
                        });
                    }
                }
                tx.upsert(row, UpsertOpts::default())?;
            }
            for id in &removed {
                let before = mirror.get(id);
                tx.delete::<MirrorResource>(id)?;
                if !baseline && let Some(before) = before {
                    let echoed = tx.get::<WriteEcho>(&echo_key(id, DELETED))?.is_some();
                    pending.push(ChangeRow {
                        key: String::new(),
                        seq: 0,
                        event_id: id.clone(),
                        uid: before.uid.clone(),
                        kind: ChangeKind::Deleted,
                        before: Some(before.projection.clone()),
                        after: None,
                        changed_fields: Vec::new(),
                        etag: None,
                        previous_etag: Some(before.etag.clone()),
                        origin: if echoed {
                            ChangeOrigin::Omni
                        } else {
                            ChangeOrigin::External
                        },
                        detected_at: now,
                        extra: Default::default(),
                    });
                }
            }
            let added = pending.len();
            for mut change in pending {
                state.change_seq += 1;
                change.seq = state.change_seq;
                change.key = change_key(change.seq);
                tx.upsert(
                    &change,
                    UpsertOpts {
                        expires_at: Some(now + CHANGE_RETENTION_MS),
                        ttl_ms: None,
                    },
                )?;
            }
            prune_changes(tx, state.change_seq)?;
            if !incomplete {
                if let Some(token) = &new_token {
                    state.sync_token = Some(token.clone());
                } else if full {
                    state.sync_token = None;
                }
            }
            state.collection_sha256 = Some(path_sha256.clone());
            state.last_attempt_at = Some(now);
            state.last_sync_at = Some(now);
            if full {
                state.last_full_sync_at = Some(now);
            }
            state.last_error = incomplete.then(|| "sync incomplete".to_owned());
            state.baselined = true;
            state.resource_count = tx.count::<MirrorResource>()?;
            tx.upsert(&state, UpsertOpts::default())?;
            Ok(added)
        })
        .await
        .map_err(PrimaryError::store)?;
    report.changes = changes;
    if changes > 0 {
        tracing::info!(target: LOG, "Detected {changes} calendar change(s)");
    }
    Ok(report)
}

/// Keeps at most [`CHANGE_MAX_ROWS`] change rows (the newest).
fn prune_changes(tx: &mut omni_store::Tx<'_>, last_seq: i64) -> Result<(), omni_store::StoreError> {
    let oldest_kept = last_seq - CHANGE_MAX_ROWS as i64;
    if oldest_kept <= 0 {
        return Ok(());
    }
    for row in tx.get_all::<ChangeRow>()? {
        if row.seq <= oldest_kept {
            tx.delete::<ChangeRow>(&row.key)?;
        }
    }
    Ok(())
}

/// Records a sync failure on the state row.
pub async fn record_failure(
    store: &Store,
    now_ms: i64,
    message: String,
) -> Result<(), PrimaryError> {
    store
        .write(move |tx| {
            let mut state = tx
                .get::<SyncState>(&SINGLETON.to_owned())?
                .unwrap_or_else(|| SyncState {
                    key: SINGLETON.to_owned(),
                    ..SyncState::default()
                });
            state.last_attempt_at = Some(now_ms);
            state.last_error = Some(message);
            tx.upsert(&state, UpsertOpts::default())
        })
        .await
        .map_err(PrimaryError::store)
}

/// Upserts one mirror row (after a fresh GET or a verified write).
pub async fn refresh_row(store: &Store, row: MirrorResource) -> Result<(), PrimaryError> {
    store
        .write(move |tx| tx.upsert(&row, UpsertOpts::default()))
        .await
        .map_err(PrimaryError::store)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_ids_stay_inside_the_collection() {
        let collection = Url::parse("https://p42-caldav.icloud.com/123/calendars/work/").unwrap();
        assert!(event_url(&collection, "ABC-1.ics").is_ok());
        assert!(event_url(&collection, "omni-abc@omni-notify.ics").is_ok());
        assert!(event_url(&collection, "../x.ics").is_err());
        assert!(event_url(&collection, "a/b.ics").is_err());
        assert!(event_url(&collection, "x.txt").is_err());
        assert_eq!(
            event_id_from_href(&collection, "/123/calendars/work/A%20B.ics").as_deref(),
            Some("A%20B.ics")
        );
        assert_eq!(
            event_id_from_href(&collection, "/123/calendars/home/a.ics"),
            None
        );
        assert_eq!(
            event_id_from_href(&collection, "/123/calendars/work/"),
            None
        );
    }
}
