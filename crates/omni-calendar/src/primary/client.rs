//! CalDAV requests for the primary calendar: bounded REPORT, GET and
//! conditional PUT/DELETE, plus multistatus parsing with quick-xml (calendar
//! data arrives entity-escaped or as CDATA, which the discovery regexes do not
//! handle).

use std::sync::{Arc, Mutex};

use omni_http::{HttpClient, HttpError, Method, RedirectRule, SideEffectMode, Url};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::caldav::RecordedCaldavWrite;
use crate::caldav::http::CALDAV_REQUEST_TIMEOUT;

/// Cap for one multistatus response.
pub const XML_MAX_BYTES: usize = 2 * 1024 * 1024;
/// Cap for one calendar resource.
pub const RESOURCE_MAX_BYTES: usize = 256 * 1024;
/// Cap for a write response.
const WRITE_RESPONSE_MAX_BYTES: usize = 64 * 1024;

/// One response, body read under its cap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DavResponse {
    pub status: u16,
    pub etag: Option<String>,
    pub location: Option<String>,
    pub body: Vec<u8>,
}

impl DavResponse {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The first 300 characters of the body (for error details).
    pub fn excerpt(&self) -> String {
        self.text().chars().take(300).collect()
    }
}

/// A read failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    #[error("response exceeds the {0}-byte limit")]
    TooLarge(usize),
    #[error("{0}")]
    Transport(String),
}

/// A write failed at the transport level.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    /// Refused locally before any byte was sent.
    #[error("not sent: {0}")]
    NotSent(String),
    /// The request may have reached the server.
    #[error("{0}")]
    Uncertain(String),
}

/// A conditional write's precondition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Precondition {
    IfMatch(String),
    IfNoneMatchAny,
}

impl Precondition {
    pub fn describe(&self) -> String {
        match self {
            Precondition::IfMatch(etag) => format!("if-match:{etag}"),
            Precondition::IfNoneMatchAny => "if-none-match".to_owned(),
        }
    }
}

/// Requests for one authenticated calendar.
#[derive(Clone)]
pub struct DavClient {
    http: HttpClient,
    auth: String,
    mode: SideEffectMode,
    recorded: Arc<Mutex<Vec<RecordedCaldavWrite>>>,
}

impl DavClient {
    pub fn new(
        http: HttpClient,
        auth: String,
        mode: SideEffectMode,
        recorded: Arc<Mutex<Vec<RecordedCaldavWrite>>>,
    ) -> Self {
        Self {
            http,
            auth,
            mode,
            recorded,
        }
    }

    async fn send(
        &self,
        method: Method,
        url: &Url,
        headers: &[(&'static str, &str)],
        body: Option<String>,
        max_bytes: usize,
    ) -> Result<DavResponse, HttpError> {
        let mut builder = self
            .http
            .request(method, url.clone())
            .timeout(CALDAV_REQUEST_TIMEOUT)
            .redirect(RedirectRule::None)
            .header("Authorization", self.auth.as_str());
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        if let Some(body) = body {
            builder = builder.body(body);
        }
        let response = builder.send_bounded(max_bytes).await?;
        let header = |name: &str| {
            response
                .headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        Ok(DavResponse {
            status: response.status.as_u16(),
            etag: header("etag"),
            location: header("location"),
            body: response.body.to_vec(),
        })
    }

    fn read_error(error: HttpError) -> ReadError {
        match error {
            HttpError::TooLarge { limit } => ReadError::TooLarge(limit),
            other => ReadError::Transport(other.to_string()),
        }
    }

    pub async fn report(
        &self,
        url: &Url,
        depth: &str,
        body: &str,
    ) -> Result<DavResponse, ReadError> {
        let method =
            Method::from_bytes(b"REPORT").map_err(|e| ReadError::Transport(e.to_string()))?;
        self.send(
            method,
            url,
            &[
                ("Content-Type", "application/xml; charset=utf-8"),
                ("Depth", depth),
            ],
            Some(body.to_owned()),
            XML_MAX_BYTES,
        )
        .await
        .map_err(Self::read_error)
    }

    pub async fn propfind(
        &self,
        url: &Url,
        depth: &str,
        body: &str,
    ) -> Result<DavResponse, ReadError> {
        let method =
            Method::from_bytes(b"PROPFIND").map_err(|e| ReadError::Transport(e.to_string()))?;
        self.send(
            method,
            url,
            &[
                ("Content-Type", "application/xml; charset=utf-8"),
                ("Depth", depth),
            ],
            Some(body.to_owned()),
            XML_MAX_BYTES,
        )
        .await
        .map_err(Self::read_error)
    }

    pub async fn get(&self, url: &Url) -> Result<DavResponse, ReadError> {
        self.send(Method::GET, url, &[], None, RESOURCE_MAX_BYTES)
            .await
            .map_err(Self::read_error)
    }

    fn record(&self, method: &'static str, url: &Url, body: Option<String>) {
        self.recorded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(RecordedCaldavWrite {
                method,
                url: url.to_string(),
                body,
            });
    }

    fn write_error(error: HttpError) -> WriteError {
        match error {
            HttpError::InvalidUrl(m) | HttpError::Blocked(m) => WriteError::NotSent(m),
            other => WriteError::Uncertain(other.to_string()),
        }
    }

    /// A conditional PUT. In record mode the write is captured and reported
    /// as `201` without an ETag.
    pub async fn put(
        &self,
        url: &Url,
        body: String,
        precondition: &Precondition,
    ) -> Result<DavResponse, WriteError> {
        if self.mode == SideEffectMode::Record {
            self.record("PUT", url, Some(body));
            return Ok(DavResponse {
                status: 201,
                etag: None,
                location: None,
                body: Vec::new(),
            });
        }
        let (name, value) = match precondition {
            Precondition::IfMatch(etag) => ("If-Match", etag.as_str()),
            Precondition::IfNoneMatchAny => ("If-None-Match", "*"),
        };
        self.send(
            Method::PUT,
            url,
            &[
                ("Content-Type", "text/calendar; charset=utf-8"),
                (name, value),
            ],
            Some(body),
            WRITE_RESPONSE_MAX_BYTES,
        )
        .await
        .map_err(Self::write_error)
    }

    /// A DELETE with `If-Match`.
    pub async fn delete(&self, url: &Url, etag: &str) -> Result<DavResponse, WriteError> {
        if self.mode == SideEffectMode::Record {
            self.record("DELETE", url, None);
            return Ok(DavResponse {
                status: 204,
                etag: None,
                location: None,
                body: Vec::new(),
            });
        }
        self.send(
            Method::DELETE,
            url,
            &[("If-Match", etag)],
            None,
            WRITE_RESPONSE_MAX_BYTES,
        )
        .await
        .map_err(Self::write_error)
    }

    pub fn is_recording(&self) -> bool {
        self.mode == SideEffectMode::Record
    }
}

/// A minimal element tree (local names, lowercased; text concatenated).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XmlNode {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<XmlNode>,
}

impl XmlNode {
    /// An attribute by local name.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn child(&self, name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a XmlNode> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The first descendant (depth first) named `name`.
    pub fn find(&self, name: &str) -> Option<&XmlNode> {
        for child in &self.children {
            if child.name == name {
                return Some(child);
            }
            if let Some(found) = child.find(name) {
                return Some(found);
            }
        }
        None
    }

    pub fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a XmlNode>) {
        for child in &self.children {
            if child.name == name {
                out.push(child);
            }
            child.find_all(name, out);
        }
    }

    pub fn trimmed(&self) -> String {
        self.text.trim().to_owned()
    }

    /// The text of the first `href` below this node.
    pub fn href(&self) -> Option<String> {
        self.find("href")
            .map(XmlNode::trimmed)
            .filter(|h| !h.is_empty())
    }
}

fn local_name(name: &str) -> String {
    name.rsplit(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn resolve_entity(name: &str) -> String {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => number.parse::<u32>().ok(),
        };
        return code
            .and_then(char::from_u32)
            .map_or_else(|| format!("&{name};"), String::from);
    }
    match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        other => return format!("&{other};"),
    }
    .to_owned()
}

fn attributes(start: &quick_xml::events::BytesStart<'_>) -> Vec<(String, String)> {
    start
        .attributes()
        .with_checks(false)
        .flatten()
        .map(|a| {
            let raw = AsRef::<str>::as_ref(&a.value).to_owned();
            let value = raw
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&");
            (local_name(AsRef::<str>::as_ref(&a.key)), value)
        })
        .collect()
}

/// Parses an XML document into a tree.
pub fn parse_xml(xml: &str) -> Result<XmlNode, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<XmlNode> = vec![XmlNode::default()];
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(start) => stack.push(XmlNode {
                name: local_name(AsRef::<str>::as_ref(&start.name())),
                attrs: attributes(&start),
                ..XmlNode::default()
            }),
            Event::Empty(start) => {
                let node = XmlNode {
                    name: local_name(AsRef::<str>::as_ref(&start.name())),
                    attrs: attributes(&start),
                    ..XmlNode::default()
                };
                if let Some(top) = stack.last_mut() {
                    top.children.push(node);
                }
            }
            Event::End(_) => {
                if stack.len() < 2 {
                    return Err("unbalanced end tag".to_owned());
                }
                if let Some(node) = stack.pop()
                    && let Some(parent) = stack.last_mut()
                {
                    parent.children.push(node);
                }
            }
            Event::Text(text) => {
                if let Some(top) = stack.last_mut() {
                    top.text
                        .push_str(&AsRef::<str>::as_ref(&text).replace("\r\n", "\n"));
                }
            }
            Event::CData(data) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(AsRef::<str>::as_ref(&data));
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(top) = stack.last_mut() {
                    top.text
                        .push_str(&resolve_entity(AsRef::<str>::as_ref(&reference)));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if stack.len() != 1 {
        return Err("unterminated element".to_owned());
    }
    stack
        .pop()
        .and_then(|root| root.children.into_iter().next())
        .ok_or_else(|| "empty document".to_owned())
}

/// `HTTP/1.1 404 Not Found` → 404.
pub fn status_code(text: &str) -> Option<u16> {
    text.split_whitespace().nth(1)?.parse().ok()
}

/// One `<response>` of a multistatus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MsResponse {
    pub href: String,
    /// The response-level status (sync-collection deletions, 507 truncation).
    pub status: Option<u16>,
    /// Properties from `200` propstats.
    pub props: Vec<XmlNode>,
}

impl MsResponse {
    pub fn prop(&self, name: &str) -> Option<&XmlNode> {
        self.props.iter().find(|p| p.name == name)
    }

    pub fn prop_text(&self, name: &str) -> Option<String> {
        self.prop(name)
            .map(XmlNode::trimmed)
            .filter(|t| !t.is_empty())
    }

    /// The raw `calendar-data` text (not trimmed).
    pub fn calendar_data(&self) -> Option<&str> {
        self.prop("calendar-data")
            .map(|p| p.text.as_str())
            .filter(|t| !t.trim().is_empty())
    }
}

/// A parsed multistatus: responses plus a top-level `sync-token`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Multistatus {
    pub responses: Vec<MsResponse>,
    pub sync_token: Option<String>,
}

pub fn parse_multistatus(xml: &str) -> Result<Multistatus, String> {
    let root = parse_xml(xml)?;
    if root.name != "multistatus" {
        return Err(format!("expected multistatus, got {}", root.name));
    }
    let mut out = Multistatus {
        sync_token: root
            .child("sync-token")
            .map(XmlNode::trimmed)
            .filter(|t| !t.is_empty()),
        ..Multistatus::default()
    };
    for response in root.children_named("response") {
        let href = response
            .child("href")
            .map(XmlNode::trimmed)
            .unwrap_or_default();
        let status = response.child("status").and_then(|s| status_code(&s.text));
        let mut props = Vec::new();
        for propstat in response.children_named("propstat") {
            let ok = propstat
                .child("status")
                .and_then(|s| status_code(&s.text))
                .is_none_or(|code| (200..300).contains(&code));
            if !ok {
                continue;
            }
            if let Some(prop) = propstat.child("prop") {
                props.extend(prop.children.iter().cloned());
            }
        }
        out.responses.push(MsResponse {
            href,
            status,
            props,
        });
    }
    Ok(out)
}

/// Whether an error body names a precondition element (e.g. `valid-sync-token`).
pub fn error_names(body: &str, element: &str) -> bool {
    parse_xml(body)
        .ok()
        .is_some_and(|root| root.name == element || root.find(element).is_some())
}

/// Escapes text for an XML element body.
pub fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_calendar_data_with_entities_and_cdata() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <d:response><d:href>/1/calendars/work/a.ics</d:href>
    <d:propstat><d:prop><d:getetag>"e1"</d:getetag>
      <C:calendar-data>BEGIN:VCALENDAR&#13;
SUMMARY:A &amp; B&#13;
END:VCALENDAR</C:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:response><d:href>/1/calendars/work/b.ics</d:href>
    <d:propstat><d:prop><d:getetag>"e2"</d:getetag><C:calendar-data><![CDATA[BEGIN:VCALENDAR
END:VCALENDAR]]></C:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:response><d:href>/1/calendars/work/c.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>
  <d:sync-token>http://example.com/sync/2</d:sync-token>
</d:multistatus>"#;
        let ms = parse_multistatus(xml).unwrap();
        assert_eq!(ms.sync_token.as_deref(), Some("http://example.com/sync/2"));
        assert_eq!(ms.responses.len(), 3);
        assert_eq!(
            ms.responses[0].prop_text("getetag").as_deref(),
            Some("\"e1\"")
        );
        assert!(
            ms.responses[0]
                .calendar_data()
                .unwrap()
                .contains("SUMMARY:A & B\r\n")
        );
        assert!(
            ms.responses[1]
                .calendar_data()
                .unwrap()
                .starts_with("BEGIN:VCALENDAR")
        );
        assert_eq!(ms.responses[2].status, Some(404));
    }

    #[test]
    fn detects_precondition_elements() {
        let body = r#"<d:error xmlns:d="DAV:"><d:valid-sync-token/></d:error>"#;
        assert!(error_names(body, "valid-sync-token"));
        assert!(!error_names(body, "no-uid-conflict"));
    }
}
