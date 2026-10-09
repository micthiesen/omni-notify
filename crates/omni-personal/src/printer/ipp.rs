//! IPP/2.0 over HTTP for the fixed LAN printer (the `@pnosolutions/ipp`
//! `Printer` the TS service used): `Get-Printer-Attributes`, `Print-Job` and
//! `Get-Job-Attributes`. Requests are encoded and responses parsed with the
//! `ipp` crate; transport is the shared HTTP client.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use bytes::Bytes;
use futures::future::BoxFuture;
use ipp::attribute::IppAttribute;
use ipp::model::{DelimiterTag, IppVersion, Operation};
use ipp::parser::IppParser;
use ipp::reader::IppReader;
use ipp::request::IppRequestResponse;
use ipp::value::IppValue;
use omni_http::{HttpClient, Method, SideEffectMode, Url};

const MAX_IPP_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const REQUESTING_USER: &str = "ipp-client";

/// A decoded attribute value, as the TS IPP library exposed it.
#[derive(Clone, Debug, PartialEq)]
pub enum IppRaw {
    Number(f64),
    Bool(bool),
    Text(String),
    Array(Vec<IppRaw>),
    Other,
}

impl IppRaw {
    fn first(&self) -> &IppRaw {
        match self {
            IppRaw::Array(items) => items.first().unwrap_or(&IppRaw::Other),
            other => other,
        }
    }

    /// `rawNumber`: the first value when it is a finite number.
    pub fn number(&self) -> Option<f64> {
        match self.first() {
            IppRaw::Number(n) if n.is_finite() => Some(*n),
            _ => None,
        }
    }

    /// `firstRawValue` as a boolean.
    pub fn boolean(&self) -> Option<bool> {
        match self.first() {
            IppRaw::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// `rawStrings`: every string value.
    pub fn strings(&self) -> Vec<String> {
        match self {
            IppRaw::Array(items) => items
                .iter()
                .filter_map(|item| match item {
                    IppRaw::Text(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
            IppRaw::Text(s) => vec![s.clone()],
            _ => Vec::new(),
        }
    }

    /// `str()`: first string, numbers in JS form, empty when absent.
    fn str(&self) -> String {
        match self.first() {
            IppRaw::Text(s) => s.clone(),
            IppRaw::Number(n) => omni_core::js::number_to_string(*n),
            IppRaw::Bool(b) => b.to_string(),
            IppRaw::Array(_) | IppRaw::Other => String::new(),
        }
    }

    /// `strArray()`.
    fn str_array(&self) -> Vec<String> {
        match self {
            IppRaw::Array(items) => items.iter().map(IppRaw::str).collect(),
            other => vec![other.str()],
        }
    }
}

fn raw_of(value: &IppValue) -> IppRaw {
    match value {
        IppValue::Integer(n) | IppValue::Enum(n) => IppRaw::Number(f64::from(*n)),
        IppValue::Boolean(b) => IppRaw::Bool(*b),
        IppValue::Array(items) => IppRaw::Array(items.iter().map(raw_of).collect()),
        IppValue::TextWithoutLanguage(_)
        | IppValue::NameWithoutLanguage(_)
        | IppValue::TextWithLanguage { .. }
        | IppValue::NameWithLanguage { .. }
        | IppValue::Charset(_)
        | IppValue::NaturalLanguage(_)
        | IppValue::Uri(_)
        | IppValue::UriScheme(_)
        | IppValue::Keyword(_)
        | IppValue::MimeMediaType(_)
        | IppValue::MemberAttrName(_) => IppRaw::Text(text_of(value)),
        _ => IppRaw::Other,
    }
}

fn text_of(value: &IppValue) -> String {
    match value {
        IppValue::TextWithLanguage { text, .. } => text.to_string(),
        IppValue::NameWithLanguage { name, .. } => name.to_string(),
        other => other.to_string(),
    }
}

/// Attributes of one group tag, last value per name wins (`getAttributes`).
pub type Attributes = BTreeMap<String, IppRaw>;

/// A decoded IPP response.
#[derive(Clone, Debug, PartialEq)]
pub struct IppResponse {
    pub status_code: u16,
    pub request_id: i32,
    groups: Vec<(DelimiterTag, Vec<(String, IppRaw)>)>,
}

impl IppResponse {
    pub fn attributes(&self, tag: DelimiterTag) -> Attributes {
        let mut out = Attributes::new();
        for (group_tag, attributes) in &self.groups {
            if *group_tag != tag {
                continue;
            }
            for (name, value) in attributes {
                out.insert(name.clone(), value.clone());
            }
        }
        out
    }

    /// `getAttribute` across all groups.
    pub fn attribute(&self, name: &str) -> Option<&IppRaw> {
        self.groups
            .iter()
            .flat_map(|(_, attributes)| attributes)
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
    }
}

/// Parses an IPP response body.
pub fn parse_response(body: &[u8]) -> Result<IppResponse, String> {
    let reader = IppReader::new(Cursor::new(body.to_vec()));
    let parsed = IppParser::new(reader)
        .parse()
        .map_err(|e| format!("Invalid IPP response: {e}"))?;
    let header = parsed.header();
    let status_code = u16::from_ne_bytes(header.operation_or_status.to_ne_bytes());
    let groups = parsed
        .attributes()
        .groups()
        .iter()
        .map(|group| {
            let attributes = group
                .attributes()
                .iter()
                .map(|attr| (attr.name().to_string(), raw_of(attr.value())))
                .collect();
            (group.tag(), attributes)
        })
        .collect();
    Ok(IppResponse {
        status_code,
        request_id: header.request_id,
        groups,
    })
}

/// Normalized `Get-Printer-Attributes` result.
#[derive(Clone, Debug, PartialEq)]
pub struct IppPrinterStatus {
    pub name: Option<String>,
    pub uri: String,
    pub state: String,
    pub state_reasons: Vec<String>,
    pub supported_formats: Vec<String>,
    pub supported_media: Vec<String>,
    pub raw: Attributes,
}

/// `Print-Job` options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintOptions {
    pub copies: u32,
    pub media: String,
    pub sides: String,
    pub color_mode: String,
    pub document_format: String,
    pub job_name: String,
    pub fit_to_page: bool,
}

/// An accepted job.
#[derive(Clone, Debug, PartialEq)]
pub struct PrintJob {
    pub id: Option<f64>,
    pub uri: String,
    pub state: String,
    pub name: String,
}

/// `Get-Job-Attributes` result.
#[derive(Clone, Debug, PartialEq)]
pub struct JobStatus {
    pub state: String,
    pub state_reasons: Vec<String>,
    pub impressions_completed: Option<f64>,
}

/// The printer operations the service needs (a seam for tests).
pub trait PrinterClient: Send + Sync {
    fn status(&self) -> BoxFuture<'_, Result<IppPrinterStatus, String>>;
    fn print<'a>(
        &'a self,
        data: Bytes,
        options: &'a PrintOptions,
    ) -> BoxFuture<'a, Result<PrintJob, String>>;
    /// `None` when the client cannot read job status.
    fn job_status<'a>(
        &'a self,
        job_uri: &'a str,
    ) -> Option<BoxFuture<'a, Result<JobStatus, String>>>;
}

/// RFC 8011 status-code names, as the TS IPP library reports them.
fn status_name(code: u16) -> &'static str {
    match code {
        0x0000 => "ok",
        0x0001 => "ok-ignored-or-substituted-attributes",
        0x0002 => "ok-conflicting-attributes",
        0x0400 => "bad-request",
        0x0401 => "forbidden",
        0x0402 => "not-authenticated",
        0x0403 => "not-authorized",
        0x0404 => "not-possible",
        0x0405 => "timeout",
        0x0406 => "not-found",
        0x0407 => "gone",
        0x0408 => "request-entity-too-large",
        0x0409 => "request-value-too-long",
        0x040a => "document-format-not-supported",
        0x040b => "attributes-not-supported",
        0x040c => "uri-scheme-not-supported",
        0x040d => "charset-not-supported",
        0x040e => "conflicting-attributes",
        0x040f => "compression-not-supported",
        0x0410 => "compression-error",
        0x0411 => "document-format-error",
        0x0412 => "document-access-error",
        0x0500 => "internal-error",
        0x0501 => "operation-not-supported",
        0x0502 => "service-unavailable",
        0x0503 => "version-not-supported",
        0x0504 => "device-error",
        0x0505 => "temporary-error",
        0x0506 => "not-accepting-jobs",
        0x0507 => "busy",
        0x0508 => "job-canceled",
        0x0509 => "multiple-document-jobs-not-supported",
        _ => "unknown",
    }
}

pub fn printer_state(state: Option<f64>) -> String {
    match state {
        None => "unknown".to_owned(),
        Some(3.0) => "idle".to_owned(),
        Some(4.0) => "processing".to_owned(),
        Some(5.0) => "stopped".to_owned(),
        Some(n) => format!("unknown:{}", omni_core::js::number_to_string(n)),
    }
}

pub fn job_state(state: Option<f64>) -> String {
    let name = match state {
        None => return "unknown".to_owned(),
        Some(3.0) => "pending",
        Some(4.0) => "pending-held",
        Some(5.0) => "processing",
        Some(6.0) => "processing-stopped",
        Some(7.0) => "canceled",
        Some(8.0) => "aborted",
        Some(9.0) => "completed",
        Some(n) => return format!("unknown:{}", omni_core::js::number_to_string(n)),
    };
    name.to_owned()
}

/// A Print-Job captured in `SideEffectMode::Record`.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedPrintJob {
    pub bytes: usize,
    pub options: PrintOptions,
}

/// The production IPP client.
pub struct IppPrinterClient {
    http: HttpClient,
    printer_uri: String,
    http_url: Url,
    /// Percent-decoded `user:password` from the printer URL (Basic auth).
    credentials: Option<(String, String)>,
    timeout: Duration,
    mode: SideEffectMode,
    request_id: AtomicI32,
    recorded: Mutex<Vec<RecordedPrintJob>>,
}

/// `ipp://host[:port]/path` → `http://host:631/path` (`ipps` → `https`).
pub fn ipp_http_url(uri: &str) -> Result<Url, String> {
    let url = Url::parse(uri).map_err(|e| format!("Invalid printer URL: {e}"))?;
    let scheme = match url.scheme() {
        "ipp" => "http",
        "ipps" => "https",
        _ => return Ok(url),
    };
    let host = url.host_str().ok_or("Printer URL has no host")?;
    let port = url.port().unwrap_or(631);
    let mut http = Url::parse(&format!("{scheme}://{host}:{port}"))
        .map_err(|e| format!("Invalid printer URL: {e}"))?;
    http.set_path(url.path());
    http.set_query(url.query());
    Ok(http)
}

/// `resolveAuthorization`: URL userinfo becomes Basic credentials, never part
/// of the request URL.
fn url_credentials(uri: &str) -> Result<Option<(String, String)>, String> {
    let url = Url::parse(uri).map_err(|e| format!("Invalid printer URL: {e}"))?;
    if url.username().is_empty() && url.password().is_none() {
        return Ok(None);
    }
    // `decodeURIComponent`, keeping the raw text when it is not valid UTF-8.
    let decode = |value: &str| {
        percent_encoding::percent_decode_str(value)
            .decode_utf8()
            .map_or_else(|_| value.to_owned(), std::borrow::Cow::into_owned)
    };
    Ok(Some((
        decode(url.username()),
        decode(url.password().unwrap_or("")),
    )))
}

fn attr(name: &str, value: IppValue) -> Result<IppAttribute, String> {
    IppAttribute::with_name(name, value).map_err(|e| e.to_string())
}

fn ipp_text<T: TryFrom<String>>(value: &str) -> Result<T, String> {
    T::try_from(value.to_owned()).map_err(|_| format!("IPP value too long: {value}"))
}

impl IppPrinterClient {
    pub fn new(
        http: HttpClient,
        printer_uri: &str,
        timeout: Duration,
        mode: SideEffectMode,
        seed: i32,
    ) -> Result<Self, String> {
        Ok(Self {
            http,
            printer_uri: printer_uri.to_owned(),
            http_url: ipp_http_url(printer_uri)?,
            credentials: url_credentials(printer_uri)?,
            timeout,
            mode,
            request_id: AtomicI32::new(seed.max(1)),
            recorded: Mutex::new(Vec::new()),
        })
    }

    /// Print jobs captured in `SideEffectMode::Record`.
    pub fn recorded(&self) -> Vec<RecordedPrintJob> {
        self.recorded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn next_request_id(&self) -> i32 {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        if id <= 0 { 1 } else { id }
    }

    fn request(
        &self,
        operation: Operation,
        extra_operation: Vec<IppAttribute>,
        job: Vec<IppAttribute>,
    ) -> Result<(IppRequestResponse, i32), String> {
        let mut request = IppRequestResponse::new(IppVersion::v2_0(), operation, None)
            .map_err(|e| e.to_string())?;
        let id = self.next_request_id();
        request.header_mut().request_id = id;
        let attributes = request.attributes_mut();
        attributes.add(
            DelimiterTag::OperationAttributes,
            attr("printer-uri", IppValue::Uri(ipp_text(&self.printer_uri)?))?,
        );
        for attribute in extra_operation {
            attributes.add(DelimiterTag::OperationAttributes, attribute);
        }
        for attribute in job {
            attributes.add(DelimiterTag::JobAttributes, attribute);
        }
        Ok((request, id))
    }

    async fn send(
        &self,
        request: IppRequestResponse,
        request_id: i32,
        data: Option<Bytes>,
    ) -> Result<IppResponse, String> {
        let mut body = request.to_bytes().to_vec();
        if let Some(data) = data {
            body.extend_from_slice(&data);
        }
        let mut request = self
            .http
            .request(Method::POST, self.http_url.clone())
            .header("content-type", "application/ipp")
            .header("accept", "application/ipp");
        if let Some((user, password)) = &self.credentials {
            request = request.basic_auth(user, Some(password.as_str()));
        }
        let response = request
            .body(body)
            .timeout(self.timeout)
            .send_bounded(MAX_IPP_RESPONSE_BYTES)
            .await
            .map_err(|e| e.to_string())?;
        if !response.status.is_success() {
            return Err(format!(
                "HTTP error: {} {}",
                response.status.as_u16(),
                response.status.canonical_reason().unwrap_or("")
            ));
        }
        if let Some(content_type) = response
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            && !content_type.starts_with("application/ipp")
        {
            return Err(format!(
                "Expected an application/ipp response, got {content_type}"
            ));
        }
        let parsed = parse_response(&response.body)?;
        if parsed.request_id != request_id && parsed.request_id != 0 {
            return Err(format!(
                "Response request-id {} does not match request request-id {request_id}",
                parsed.request_id
            ));
        }
        Ok(parsed)
    }

    fn ensure_success(response: &IppResponse) -> Result<(), String> {
        if response.status_code >= 0x0400 {
            let message = response
                .attribute("status-message")
                .map(IppRaw::str)
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| {
                    format!(
                        "IPP operation error (0x{:x} - {})",
                        response.status_code,
                        status_name(response.status_code)
                    )
                });
            return Err(message);
        }
        Ok(())
    }
}

impl PrinterClient for IppPrinterClient {
    fn status(&self) -> BoxFuture<'_, Result<IppPrinterStatus, String>> {
        Box::pin(async move {
            let (request, id) = self.request(
                Operation::GetPrinterAttributes,
                vec![
                    attr(
                        "requesting-user-name",
                        IppValue::NameWithoutLanguage(ipp_text(REQUESTING_USER)?),
                    )?,
                    attr("requested-attributes", IppValue::Keyword(ipp_text("all")?))?,
                ],
                Vec::new(),
            )?;
            let response = self.send(request, id, None).await?;
            Self::ensure_success(&response)?;
            let attrs = response.attributes(DelimiterTag::PrinterAttributes);
            let get = |name: &str| attrs.get(name);
            let name = get("printer-name").and_then(|v| match v {
                IppRaw::Array(items) if items.is_empty() => None,
                other => Some(other.str()),
            });
            let uri = get("printer-uri-supported")
                .map(IppRaw::str)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| self.printer_uri.clone());
            Ok(IppPrinterStatus {
                name,
                uri,
                state: printer_state(get("printer-state").and_then(IppRaw::number)),
                state_reasons: get("printer-state-reasons")
                    .map(IppRaw::str_array)
                    .unwrap_or_default(),
                supported_formats: get("document-format-supported")
                    .map(IppRaw::str_array)
                    .unwrap_or_default(),
                supported_media: get("media-supported")
                    .map(IppRaw::str_array)
                    .unwrap_or_default(),
                raw: attrs.clone(),
            })
        })
    }

    fn print<'a>(
        &'a self,
        data: Bytes,
        options: &'a PrintOptions,
    ) -> BoxFuture<'a, Result<PrintJob, String>> {
        Box::pin(async move {
            if self.mode == SideEffectMode::Record {
                self.recorded
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(RecordedPrintJob {
                        bytes: data.len(),
                        options: options.clone(),
                    });
                return Err(
                    "Print-Job recorded in side-effect record mode; nothing was sent to the printer"
                        .to_owned(),
                );
            }
            let mut job = Vec::new();
            if options.copies > 1 {
                job.push(attr(
                    "copies",
                    IppValue::Integer(i32::try_from(options.copies).map_err(|e| e.to_string())?),
                )?);
            }
            job.push(attr("media", IppValue::Keyword(ipp_text(&options.media)?))?);
            job.push(attr("sides", IppValue::Keyword(ipp_text(&options.sides)?))?);
            job.push(attr(
                "print-color-mode",
                IppValue::Keyword(ipp_text(&options.color_mode)?),
            )?);
            if options.fit_to_page {
                job.push(attr("print-scaling", IppValue::Keyword(ipp_text("fit")?))?);
            }
            let (request, id) = self.request(
                Operation::PrintJob,
                vec![
                    attr(
                        "requesting-user-name",
                        IppValue::NameWithoutLanguage(ipp_text(REQUESTING_USER)?),
                    )?,
                    attr(
                        "job-name",
                        IppValue::NameWithoutLanguage(ipp_text(&options.job_name)?),
                    )?,
                    attr(
                        "document-format",
                        IppValue::MimeMediaType(ipp_text(&options.document_format)?),
                    )?,
                ],
                job,
            )?;
            let response = self.send(request, id, Some(data)).await?;
            Self::ensure_success(&response)?;
            let job_attrs = response.attributes(DelimiterTag::JobAttributes);
            // `numOrNull(job[name]) ?? numOrNull(getAttribute(response, name))`.
            let number = |name: &str| {
                job_attrs
                    .get(name)
                    .and_then(IppRaw::number)
                    .or_else(|| response.attribute(name).and_then(IppRaw::number))
            };
            // `str(job[name]) || str(getAttribute(response, name)) || ""`.
            let text = |name: &str| {
                job_attrs
                    .get(name)
                    .map(IppRaw::str)
                    .filter(|s| !s.is_empty())
                    .or_else(|| response.attribute(name).map(IppRaw::str))
                    .unwrap_or_default()
            };
            Ok(PrintJob {
                id: number("job-id"),
                uri: text("job-uri"),
                state: job_state(number("job-state")),
                name: options.job_name.clone(),
            })
        })
    }

    fn job_status<'a>(
        &'a self,
        job_uri: &'a str,
    ) -> Option<BoxFuture<'a, Result<JobStatus, String>>> {
        Some(Box::pin(async move {
            let (mut request, id) =
                self.request(Operation::GetJobAttributes, Vec::new(), Vec::new())?;
            // Get-Job-Attributes addresses the job by `job-uri`, not `printer-uri`.
            let operation = request
                .attributes_mut()
                .groups_mut()
                .iter_mut()
                .find(|g| g.tag() == DelimiterTag::OperationAttributes)
                .ok_or("missing operation attributes")?;
            operation
                .attributes_mut()
                .retain(|a| a.name().as_str() != "printer-uri");
            operation
                .attributes_mut()
                .push(attr("job-uri", IppValue::Uri(ipp_text(job_uri)?))?);
            operation.attributes_mut().push(attr(
                "requested-attributes",
                IppValue::Array(vec![
                    IppValue::Keyword(ipp_text("job-state")?),
                    IppValue::Keyword(ipp_text("job-state-reasons")?),
                    IppValue::Keyword(ipp_text("job-impressions-completed")?),
                ]),
            )?);
            let response = self.send(request, id, None).await?;
            if response.status_code >= 0x0400 {
                return Err(format!(
                    "Could not read printer job status (IPP 0x{:x})",
                    response.status_code
                ));
            }
            let attrs = response.attributes(DelimiterTag::JobAttributes);
            Ok(JobStatus {
                state: job_state(attrs.get("job-state").and_then(IppRaw::number)),
                state_reasons: attrs
                    .get("job-state-reasons")
                    .map(IppRaw::strings)
                    .unwrap_or_default(),
                impressions_completed: attrs
                    .get("job-impressions-completed")
                    .and_then(IppRaw::number)
                    .filter(|n| *n >= 0.0),
            })
        }))
    }
}
