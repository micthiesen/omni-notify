//! Outgoing mail (`src/emails/*`, ARCHITECTURE.md section 3.7).
//!
//! Every message is sent as [`OUTGOING_EMAIL_FROM`]; SMTP authentication
//! identities stay separate. Submission never reports success unless every
//! recipient was accepted (lettre aborts the transaction on the first RCPT
//! rejection, so a partial rejection is an error).

use std::fmt;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport as _, Tokio1Executor};
use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address as MimeAddress;
use mail_builder::headers::message_id::MessageId;
use omni_config::Config;
use omni_http::SideEffectMode;

pub use omni_config::OUTGOING_EMAIL_FROM;

pub mod templates;

const LOG: &str = "Email";

/// iCloud's submission endpoint (STARTTLS).
pub const ICLOUD_SMTP_HOST: &str = "smtp.mail.me.com";
pub const ICLOUD_SMTP_PORT: u16 = 587;

/// SMTP submission settings. `Debug` never prints credentials.
#[derive(Clone, PartialEq, Eq)]
pub enum SmtpConfig {
    /// `SMTP_HOST` + `SMTP_USER` + `SMTP_PASS`; implicit TLS on port 465, else STARTTLS.
    Explicit {
        host: String,
        port: u16,
        user: String,
        pass: String,
        implicit_tls: bool,
    },
    /// iCloud fallback, used only when no `SMTP_*` field is set.
    ICloud { user: String, app_password: String },
}

impl fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SmtpConfig::Explicit {
                host,
                port,
                implicit_tls,
                ..
            } => f
                .debug_struct("Explicit")
                .field("host", host)
                .field("port", port)
                .field("implicit_tls", implicit_tls)
                .finish_non_exhaustive(),
            SmtpConfig::ICloud { .. } => f.debug_struct("ICloud").finish_non_exhaustive(),
        }
    }
}

/// The configuration values compose resolution reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComposeValues {
    pub smtp_host: Option<String>,
    /// `SMTP_PORT` as decoded (finite number, default 587).
    pub smtp_port: f64,
    pub smtp_user: Option<String>,
    pub smtp_pass: Option<String>,
    pub icloud_username: Option<String>,
    pub icloud_app_password: Option<String>,
}

/// `resolveComposeEmailConfiguration` over [`Config`].
pub fn resolve_compose_config(c: &Config) -> Option<SmtpConfig> {
    resolve_compose_values(&ComposeValues {
        smtp_host: c.smtp_host.clone(),
        smtp_port: c.smtp_port,
        smtp_user: c.smtp_user.clone(),
        smtp_pass: c.smtp_pass.clone(),
        icloud_username: c.icloud_username.clone(),
        icloud_app_password: c.icloud_app_password.clone(),
    })
}

/// `resolveComposeEmailConfiguration`: complete explicit settings win; any
/// partial `SMTP_*` disables sending; with none set, iCloud credentials are used.
/// A `SMTP_PORT` that is not a valid TCP port also disables sending.
pub fn resolve_compose_values(c: &ComposeValues) -> Option<SmtpConfig> {
    let present = |value: &Option<String>| value.as_deref().is_some_and(|v| !v.is_empty());
    if let (Some(host), Some(user), Some(pass)) = (&c.smtp_host, &c.smtp_user, &c.smtp_pass)
        && !host.is_empty()
        && !user.is_empty()
        && !pass.is_empty()
    {
        let port = valid_port(c.smtp_port)?;
        return Some(SmtpConfig::Explicit {
            host: host.clone(),
            port,
            user: user.clone(),
            pass: pass.clone(),
            implicit_tls: port == 465,
        });
    }
    if present(&c.smtp_host) || present(&c.smtp_user) || present(&c.smtp_pass) {
        return None;
    }
    match (&c.icloud_username, &c.icloud_app_password) {
        (Some(user), Some(app_password)) if !user.is_empty() && !app_password.is_empty() => {
            Some(SmtpConfig::ICloud {
                user: user.clone(),
                app_password: app_password.clone(),
            })
        }
        _ => None,
    }
}

fn valid_port(port: f64) -> Option<u16> {
    if port.fract() != 0.0 || !(1.0..=65_535.0).contains(&port) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(port as u16)
}

/// `ComposedEmailParams`: a composed message (the sender is never an input).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComposeInput {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

/// The same identity, body and date for SMTP (`wire`, no Bcc) and the private
/// Sent copy (`content`, Bcc kept).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedEmail {
    pub from: String,
    pub date_iso: String,
    pub wire_b64: String,
    pub content_b64: String,
    pub message_id: String,
}

/// `prepareComposedEmailEffect`: builds the SMTP wire MIME (no `Bcc`
/// header) and the private Sent copy (with `Bcc`) from the same identity,
/// body, date and Message-ID. With `in_reply_to`, it is appended to
/// `references` (deduplicated) to extend the reply chain.
pub fn prepare_composed_email(
    input: &ComposeInput,
    message_id: &str,
    date_ms: i64,
) -> Result<PreparedEmail, MailError> {
    let wire = compose(input, message_id, date_ms, false)?;
    let content = compose(input, message_id, date_ms, true)?;
    Ok(PreparedEmail {
        from: OUTGOING_EMAIL_FROM.to_owned(),
        date_iso: omni_core::js::to_iso_string(date_ms),
        wire_b64: STANDARD.encode(wire),
        content_b64: STANDARD.encode(content),
        message_id: message_id.to_owned(),
    })
}

/// `<id>` or `id` -> `id` (mail-builder adds the brackets).
fn bare_id(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_owned()
}

fn address_list(addresses: &[String]) -> MimeAddress<'static> {
    MimeAddress::new_list(
        addresses
            .iter()
            .map(|a| MimeAddress::new_address(None::<String>, a.clone()))
            .collect(),
    )
}

/// `[...new Set(items)]`: first occurrence wins.
fn dedupe(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn compose(
    input: &ComposeInput,
    message_id: &str,
    date_ms: i64,
    keep_bcc: bool,
) -> Result<Vec<u8>, MailError> {
    let references = match &input.in_reply_to {
        Some(parent) => dedupe(input.references.iter().cloned().chain([parent.clone()])),
        None => input.references.clone(),
    };
    let mut builder = MessageBuilder::new()
        .from(MimeAddress::new_address(
            None::<String>,
            OUTGOING_EMAIL_FROM,
        ))
        .message_id(bare_id(message_id))
        .date(date_ms.div_euclid(1000))
        .subject(input.subject.clone());
    if !input.to.is_empty() {
        builder = builder.to(address_list(&input.to));
    }
    if !input.cc.is_empty() {
        builder = builder.cc(address_list(&input.cc));
    }
    if keep_bcc && !input.bcc.is_empty() {
        builder = builder.bcc(address_list(&input.bcc));
    }
    if let Some(parent) = &input.in_reply_to {
        builder = builder.in_reply_to(bare_id(parent));
    }
    if !references.is_empty() {
        builder = builder.references(MessageId::new_list(references.iter().map(|r| bare_id(r))));
    }
    builder
        .text_body(input.text.clone())
        .write_to_vec()
        .map_err(|e| MailError::Mime(e.to_string()))
}

/// Verifies the fixed identity of prebuilt MIME: exactly one `From` address,
/// equal to [`OUTGOING_EMAIL_FROM`], and no `Sender` or `Resent-From`.
pub fn verify_sender_identity(raw: &[u8]) -> Result<(), MailError> {
    let message = mail_parser::MessageParser::default()
        .parse(raw)
        .ok_or_else(|| MailError::Identity("unparseable MIME".to_owned()))?;
    let from: Vec<&str> = message
        .from()
        .map(|from| from.iter().filter_map(|addr| addr.address()).collect())
        .unwrap_or_default();
    if from != [OUTGOING_EMAIL_FROM] {
        return Err(MailError::Identity(format!(
            "persisted email must be from {OUTGOING_EMAIL_FROM}"
        )));
    }
    let forbidden = message.headers().iter().any(|header| {
        let name = header.name.as_str();
        name.eq_ignore_ascii_case("sender") || name.eq_ignore_ascii_case("resent-from")
    });
    if forbidden {
        return Err(MailError::Identity(
            "persisted email must not carry Sender or Resent-From".to_owned(),
        ));
    }
    Ok(())
}

/// Per-recipient SMTP outcome.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SendReport {
    pub accepted: Vec<String>,
    pub rejected: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("SMTP is not configured to send as {OUTGOING_EMAIL_FROM}")]
    NotConfigured,
    #[error("MIME composition failed: {0}")]
    Mime(String),
    #[error("outgoing message identity rejected: {0}; SMTP submission refused")]
    Identity(String),
    #[error("invalid recipient address {0:?}")]
    InvalidRecipient(String),
    /// SMTP failed or rejected a recipient. Whether the message was
    /// delivered may be unknown; callers never retry automatically.
    #[error("SMTP failed: {message}")]
    Smtp { message: String, transient: bool },
}

type Transport = AsyncSmtpTransport<Tokio1Executor>;

/// SMTP sender. In `SideEffectMode::Record` nothing is transmitted.
#[derive(Clone)]
pub struct Mailer {
    inner: Arc<MailerInner>,
}

struct MailerInner {
    cfg: SmtpConfig,
    mode: SideEffectMode,
    transport: Result<Transport, String>,
    recorded: Mutex<Vec<(Vec<String>, Vec<u8>)>>,
}

impl fmt::Debug for Mailer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mailer")
            .field("cfg", &self.inner.cfg)
            .field("mode", &self.inner.mode)
            .finish_non_exhaustive()
    }
}

fn build_transport(cfg: &SmtpConfig) -> Result<Transport, String> {
    let built = match cfg {
        SmtpConfig::Explicit {
            host,
            port,
            user,
            pass,
            implicit_tls,
        } => {
            let builder = if *implicit_tls {
                Transport::relay(host)
            } else {
                Transport::starttls_relay(host)
            };
            builder.map(|b| {
                b.port(*port)
                    .credentials(Credentials::new(user.clone(), pass.clone()))
                    .build()
            })
        }
        SmtpConfig::ICloud { user, app_password } => Transport::starttls_relay(ICLOUD_SMTP_HOST)
            .map(|b| {
                b.port(ICLOUD_SMTP_PORT)
                    .credentials(Credentials::new(user.clone(), app_password.clone()))
                    .build()
            }),
    };
    built.map_err(|e| e.to_string())
}

impl Mailer {
    pub fn new(cfg: SmtpConfig, mode: SideEffectMode) -> Self {
        let transport = build_transport(&cfg);
        Self::with_transport(cfg, mode, transport)
    }

    /// An unauthenticated plaintext SMTP client for a local fake server.
    /// Never use in production wiring.
    pub fn plaintext_for_tests(host: &str, port: u16, mode: SideEffectMode) -> Self {
        let cfg = SmtpConfig::Explicit {
            host: host.to_owned(),
            port,
            user: String::new(),
            pass: String::new(),
            implicit_tls: false,
        };
        let transport = Transport::builder_dangerous(host).port(port).build();
        Self::with_transport(cfg, mode, Ok(transport))
    }

    fn with_transport(
        cfg: SmtpConfig,
        mode: SideEffectMode,
        transport: Result<Transport, String>,
    ) -> Self {
        Self {
            inner: Arc::new(MailerInner {
                cfg,
                mode,
                transport,
                recorded: Mutex::new(Vec::new()),
            }),
        }
    }

    /// The resolved submission settings.
    pub fn config(&self) -> &SmtpConfig {
        &self.inner.cfg
    }

    /// Whether this mailer records instead of sending.
    pub fn mode(&self) -> SideEffectMode {
        self.inner.mode
    }

    /// Raw messages captured in `SideEffectMode::Record` (recipients, MIME), oldest first.
    pub fn recorded(&self) -> Vec<(Vec<String>, Vec<u8>)> {
        self.inner
            .recorded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// `sendEmailEffect`: an HTML + text notification (log digests) from the
    /// fixed identity.
    pub async fn send_notification(
        &self,
        to: &str,
        subject: &str,
        html: &str,
        text: &str,
    ) -> Result<(), MailError> {
        let raw = MessageBuilder::new()
            .from(MimeAddress::new_address(
                None::<String>,
                OUTGOING_EMAIL_FROM,
            ))
            .to(MimeAddress::new_address(None::<String>, to.to_owned()))
            .subject(subject.to_owned())
            .text_body(text.to_owned())
            .html_body(html.to_owned())
            .write_to_vec()
            .map_err(|e| MailError::Mime(e.to_string()))?;
        self.submit(&[to.to_owned()], &raw).await?;
        tracing::debug!(target: LOG, "Email sent: \"{subject}\" to {to}");
        Ok(())
    }

    /// `sendComposedEmailEffect`: composes `input` and submits the Bcc-free
    /// wire form to every To, Cc and Bcc recipient (deduplicated).
    pub async fn send_composed(
        &self,
        input: &ComposeInput,
        message_id: &str,
        date_ms: i64,
    ) -> Result<SendReport, MailError> {
        let prepared = prepare_composed_email(input, message_id, date_ms)?;
        let wire = STANDARD
            .decode(&prepared.wire_b64)
            .map_err(|e| MailError::Mime(e.to_string()))?;
        let recipients: Vec<String> = input
            .to
            .iter()
            .chain(&input.cc)
            .chain(&input.bcc)
            .cloned()
            .collect();
        self.send_raw(&recipients, &wire).await
    }

    /// Submits prebuilt MIME after verifying a single `From` equal to
    /// [`OUTGOING_EMAIL_FROM`] and no `Sender`/`Resent-From`. The envelope is
    /// the fixed sender and the deduplicated `recipients` (Bcc included).
    pub async fn send_raw(
        &self,
        recipients: &[String],
        raw: &[u8],
    ) -> Result<SendReport, MailError> {
        verify_sender_identity(raw)?;
        let recipients = dedupe(recipients.iter().cloned());
        self.submit(&recipients, raw).await?;
        Ok(SendReport {
            accepted: recipients,
            rejected: Vec::new(),
        })
    }

    async fn submit(&self, recipients: &[String], raw: &[u8]) -> Result<(), MailError> {
        let from: lettre::Address = OUTGOING_EMAIL_FROM
            .parse()
            .map_err(|_| MailError::Identity(OUTGOING_EMAIL_FROM.to_owned()))?;
        let to = recipients
            .iter()
            .map(|r| {
                r.parse::<lettre::Address>()
                    .map_err(|_| MailError::InvalidRecipient(r.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let envelope = lettre::address::Envelope::new(Some(from), to)
            .map_err(|e| MailError::InvalidRecipient(e.to_string()))?;
        if self.inner.mode == SideEffectMode::Record {
            self.inner
                .recorded
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((recipients.to_vec(), raw.to_vec()));
            return Ok(());
        }
        let transport = self.inner.transport.as_ref().map_err(|e| MailError::Smtp {
            message: format!("SMTP transport unavailable: {e}"),
            transient: false,
        })?;
        transport
            .send_raw(&envelope, raw)
            .await
            .map(|_| ())
            .map_err(|e| MailError::Smtp {
                transient: e.is_transient() || e.is_timeout(),
                message: smtp_error_message(&e),
            })
    }
}

fn smtp_error_message(error: &lettre::transport::smtp::Error) -> String {
    let mut out = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        let text = cause.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = cause.source();
    }
    out
}
