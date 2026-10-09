//! CalDAV: iCloud discovery (RFC 6764), bounded HTTP, multistatus parsing,
//! iCalendar bodies and event writes.

pub mod api;
pub mod http;
pub mod ics;
pub mod merge;
pub mod xml;

use omni_config::Config;
use omni_core::clock::SharedClock;
use omni_http::{HttpClient, SideEffectMode, Url};

pub use api::{
    CaldavSession, CaldavWriter, CreateOutcome, DeleteOutcome, RecordedCaldavWrite, UpdateOutcome,
};

use crate::error::CaldavError;
use http::{assert_trusted_caldav_url, basic_auth, propfind};
use xml::{extract_calendar_collections, extract_property_href, pick_calendar_collection};

const LOG: &str = "CalDAV";

/// The well-known iCloud CalDAV root; it redirects to the account's shard.
pub const ICLOUD_CALDAV_ROOT: &str = "https://caldav.icloud.com/";

/// The only provider (`CaldavProviderName`).
pub const PROVIDER_ICLOUD: &str = "icloud";

/// iCloud CalDAV credentials and calendar selection.
#[derive(Clone)]
pub struct CaldavSettings {
    pub username: String,
    pub password: String,
    /// `ICLOUD_CALENDAR_URL`: a previously discovered collection; skips discovery.
    pub calendar_url: Option<String>,
    /// `ICLOUD_CALENDAR_NAME`: picks the collection by display name.
    pub calendar_name: Option<String>,
}

impl std::fmt::Debug for CaldavSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaldavSettings")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("calendar_url", &self.calendar_url)
            .field("calendar_name", &self.calendar_name)
            .finish()
    }
}

impl CaldavSettings {
    /// `Some` when both `ICLOUD_USERNAME` and `ICLOUD_APP_PASSWORD` are set
    /// (`getCaldavProvider`).
    pub fn from_config(config: &Config) -> Option<Self> {
        let username = config.icloud_username.clone().filter(|v| !v.is_empty())?;
        let password = config
            .icloud_app_password
            .clone()
            .filter(|v| !v.is_empty())?;
        Some(Self {
            username,
            password,
            calendar_url: config.icloud_calendar_url.clone().filter(|v| !v.is_empty()),
            calendar_name: config
                .icloud_calendar_name
                .clone()
                .filter(|v| !v.is_empty()),
        })
    }
}

/// CalDAV access for the pipeline, MCP tools and the `CalendarWriter` port.
#[derive(Clone)]
pub struct Caldav {
    http: HttpClient,
    settings: Option<CaldavSettings>,
    writer: CaldavWriter,
}

impl Caldav {
    pub fn new(
        http: HttpClient,
        clock: SharedClock,
        mode: SideEffectMode,
        settings: Option<CaldavSettings>,
        default_tz: impl Into<String>,
    ) -> Self {
        Self {
            writer: CaldavWriter::new(http.clone(), clock, mode, default_tz),
            http,
            settings,
        }
    }

    /// `"icloud"` when credentials are configured.
    pub fn provider(&self) -> Option<&'static str> {
        self.settings.as_ref().map(|_| PROVIDER_ICLOUD)
    }

    pub fn writer(&self) -> &CaldavWriter {
        &self.writer
    }

    /// Resolves the calendar collection to write to.
    pub async fn discover(&self) -> Result<CaldavSession, CaldavError> {
        let Some(settings) = &self.settings else {
            return Err(CaldavError::new(
                "discover calendar",
                "No CalDAV provider configured",
                false,
            ));
        };
        discover_icloud_calendar(&self.http, settings).await
    }
}

fn resolve(base: &Url, href: &str, operation: &str) -> Result<Url, CaldavError> {
    base.join(href)
        .map_err(|e| CaldavError::new(operation, format!("invalid href {href:?}: {e}"), false))
}

/// iCloud CalDAV discovery: the well-known root redirects to a per-account
/// shard (`https://pXX-caldav.icloud.com/<dsid>/...`), so the RFC 6764 chain
/// current-user-principal → calendar-home-set → calendar collections is
/// followed and the shard is never hardcoded. Relative hrefs resolve against
/// the URL that answered. `ICLOUD_CALENDAR_URL` short-circuits everything.
pub async fn discover_icloud_calendar(
    http: &HttpClient,
    settings: &CaldavSettings,
) -> Result<CaldavSession, CaldavError> {
    let auth_header = basic_auth(&settings.username, &settings.password);
    if let Some(configured) = &settings.calendar_url {
        let configured = if configured.ends_with('/') {
            configured.clone()
        } else {
            format!("{configured}/")
        };
        let url = assert_trusted_caldav_url(&configured)?;
        tracing::info!(target: LOG, "Using configured iCloud calendar: {url}");
        return Ok(CaldavSession {
            calendar_url: url.to_string(),
            auth_header,
        });
    }

    let root = Url::parse(ICLOUD_CALDAV_ROOT)
        .map_err(|e| CaldavError::new("discover iCloud principal", e.to_string(), false))?;

    // 1. Principal discovery at the well-known root (redirects to the shard).
    let principal = propfind(
        http,
        &root,
        &auth_header,
        "0",
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:">
  <d:prop><d:current-user-principal/></d:prop>
</d:propfind>"#,
    )
    .await?;
    let Some(principal_href) = extract_property_href(&principal.xml, "current-user-principal")
    else {
        return Err(CaldavError::new(
            "discover iCloud principal",
            "no current-user-principal in PROPFIND response",
            false,
        ));
    };
    let principal_url = assert_trusted_caldav_url(
        resolve(&principal.url, &principal_href, "discover iCloud principal")?.as_str(),
    )?;
    tracing::debug!(target: LOG, "iCloud CalDAV principal: {principal_url}");

    // 2. Calendar home on the principal.
    let home = propfind(
        http,
        &principal_url,
        &auth_header,
        "0",
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop><c:calendar-home-set/></d:prop>
</d:propfind>"#,
    )
    .await?;
    let Some(home_href) = extract_property_href(&home.xml, "calendar-home-set") else {
        return Err(CaldavError::new(
            "discover iCloud calendar home",
            "no calendar-home-set on principal",
            false,
        ));
    };
    let home_url = assert_trusted_caldav_url(
        resolve(&home.url, &home_href, "discover iCloud calendar home")?.as_str(),
    )?;
    tracing::debug!(target: LOG, "iCloud CalDAV calendar home: {home_url}");

    // 3. List collections; only VEVENT-capable calendars qualify (iCloud exposes
    //    reminder/task lists in the same home).
    let list = propfind(
        http,
        &home_url,
        &auth_header,
        "1",
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:displayname/>
    <d:resourcetype/>
    <c:supported-calendar-component-set/>
  </d:prop>
</d:propfind>"#,
    )
    .await?;
    let collections = extract_calendar_collections(&list.xml);
    let Some(selected) = pick_calendar_collection(&collections, settings.calendar_name.as_deref())
    else {
        let names = collections
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let names = if names.is_empty() {
            "none".to_owned()
        } else {
            names
        };
        return Err(CaldavError::new(
            "discover iCloud calendar collection",
            format!("no usable calendar collection found (saw: {names})"),
            false,
        ));
    };
    let calendar_url = assert_trusted_caldav_url(
        resolve(
            &list.url,
            &selected.href,
            "discover iCloud calendar collection",
        )?
        .as_str(),
    )?;
    tracing::info!(
        target: LOG,
        "Using iCloud calendar: {} ({calendar_url})",
        selected.name
    );
    Ok(CaldavSession {
        calendar_url: calendar_url.to_string(),
        auth_header,
    })
}
