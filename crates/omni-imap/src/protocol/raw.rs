//! A thin IMAP4rev1 client over any async byte stream, parsing responses with
//! `imap-proto`. It frames responses itself (lines plus `{n}` literals) so an
//! unparseable server line is skipped instead of poisoning the session, and it
//! surfaces COPYUID response codes from both untagged and tagged OK responses.

use std::borrow::Cow;
use std::collections::HashSet;
use std::time::Duration;

use futures::future::BoxFuture;
use imap_proto::types::{
    AttributeValue, Capability, MailboxDatum, NameAttribute, Response, ResponseCode, Status,
    StatusAttribute, UidSetMember,
};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::Notify;

use super::special_use::{ListRow, resolve};
use super::utf7;
use super::{
    CopyUid, FetchQuery, FetchedMessage, FolderStatus, IdleEnd, ImapClient, ImapError, ImapResult,
    MailboxEvent, MailboxInfo, SearchCriteria, SelectedMailbox, SourceRange, UidSet,
};

const LOG: &str = "IMAP";
/// imapflow's default socket timeout.
pub const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// A single response may not exceed this (the largest legitimate one is a
/// 20 MiB source fetch plus framing).
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// One part of a command line.
enum Part {
    Text(Vec<u8>),
    Literal(Vec<u8>),
}

fn text(s: impl AsRef<str>) -> Part {
    Part::Text(s.as_ref().as_bytes().to_vec())
}

/// A quoted string, or a literal when the value cannot be quoted.
fn astring(value: &str) -> Part {
    let quotable = value
        .bytes()
        .all(|b| b.is_ascii() && !matches!(b, b'\r' | b'\n' | 0));
    if quotable {
        let mut out = String::with_capacity(value.len() + 2);
        out.push('"');
        for c in value.chars() {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
        Part::Text(out.into_bytes())
    } else {
        Part::Literal(value.as_bytes().to_vec())
    }
}

fn mailbox(path: &str) -> Part {
    astring(&utf7::encode(path))
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// imapflow `formatDate`: the UTC calendar date as `DD-Mon-YYYY`.
pub fn format_search_date(ms: i64) -> String {
    let iso = omni_core::js::to_iso_string(ms);
    let year = &iso[..4];
    let month: usize = iso[5..7].parse().unwrap_or(1);
    let day = &iso[8..10];
    format!("{day}-{}-{year}", MONTHS[month.saturating_sub(1).min(11)])
}

/// imapflow `formatDateTime` for APPEND: `" D-Mon-YYYY HH:MM:SS +0000"`.
pub fn format_append_date(ms: i64) -> String {
    let date = format_search_date(ms);
    let date = match date.strip_prefix('0') {
        Some(rest) => format!(" {rest}"),
        None => date,
    };
    let iso = omni_core::js::to_iso_string(ms);
    format!("{date} {} +0000", &iso[11..19])
}

/// Parses an INTERNALDATE (`DD-Mon-YYYY HH:MM:SS +ZZZZ`) to epoch ms.
pub fn parse_internal_date(value: &str) -> Option<i64> {
    let value = value.trim();
    let (date, rest) = value.split_once(' ')?;
    let mut date_parts = date.split('-');
    let day: i8 = date_parts.next()?.trim().parse().ok()?;
    let month_name = date_parts.next()?;
    let month = MONTHS
        .iter()
        .position(|m| m.eq_ignore_ascii_case(month_name))?;
    let year: i16 = date_parts.next()?.parse().ok()?;
    let mut rest = rest.split_whitespace();
    let time = rest.next()?;
    let zone = rest.next().unwrap_or("+0000");
    let mut hms = time.split(':');
    let hour: i8 = hms.next()?.parse().ok()?;
    let minute: i8 = hms.next()?.parse().ok()?;
    let second: i8 = hms.next().unwrap_or("0").parse().ok()?;
    let sign = match zone.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits: i32 = zone.get(1..5)?.parse().ok()?;
    let offset_secs = sign * ((digits / 100) * 3600 + (digits % 100) * 60);
    let month = i8::try_from(month + 1).ok()?;
    let civil = jiff::civil::Date::new(year, month, day)
        .ok()?
        .at(hour, minute, second, 0);
    let ts = jiff::tz::Offset::from_seconds(offset_secs)
        .ok()?
        .to_timestamp(civil)
        .ok()?;
    Some(ts.as_millisecond())
}

/// `Date.now()`: imapflow measures WITHIN intervals against the wall clock.
fn system_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// `Math.round(Math.max(0, now - at) / 1000)`.
pub fn within_seconds(now_ms: i64, at_ms: i64) -> i64 {
    let elapsed = now_ms.saturating_sub(at_ms).max(0);
    (elapsed + 500).div_euclid(1000)
}

fn uid_set(set: &UidSet) -> String {
    match set {
        UidSet::From(uid) => format!("{uid}:*"),
        UidSet::List(uids) => {
            let mut sorted: Vec<u32> = uids.clone();
            sorted.sort_unstable();
            sorted.dedup();
            let mut out: Vec<String> = Vec::new();
            let mut i = 0;
            while i < sorted.len() {
                let start = sorted[i];
                let mut end = start;
                while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
                    end += 1;
                    i += 1;
                }
                out.push(if start == end {
                    start.to_string()
                } else {
                    format!("{start}:{end}")
                });
                i += 1;
            }
            out.join(",")
        }
    }
}

fn expand(members: &[UidSetMember]) -> Vec<u32> {
    members
        .iter()
        .flat_map(|m| match m {
            UidSetMember::Uid(uid) => vec![*uid],
            UidSetMember::UidRange(range) => {
                let (a, b) = (*range.start(), *range.end());
                if a <= b {
                    (a..=b).collect()
                } else {
                    (b..=a).rev().collect()
                }
            }
        })
        .collect()
}

fn copy_uid(code: &ResponseCode<'_>) -> Option<CopyUid> {
    if let ResponseCode::CopyUid(validity, source, destination) = code {
        let source = expand(source);
        let destination = expand(destination);
        if source.len() != destination.len() {
            return None;
        }
        return Some(CopyUid {
            uid_validity: *validity,
            uid_map: source.into_iter().zip(destination).collect(),
        });
    }
    None
}

fn name_attribute(attribute: &NameAttribute<'_>) -> String {
    match attribute {
        NameAttribute::NoInferiors => "\\Noinferiors".to_owned(),
        NameAttribute::NoSelect => "\\Noselect".to_owned(),
        NameAttribute::Marked => "\\Marked".to_owned(),
        NameAttribute::Unmarked => "\\Unmarked".to_owned(),
        NameAttribute::All => "\\All".to_owned(),
        NameAttribute::Archive => "\\Archive".to_owned(),
        NameAttribute::Drafts => "\\Drafts".to_owned(),
        NameAttribute::Flagged => "\\Flagged".to_owned(),
        NameAttribute::Junk => "\\Junk".to_owned(),
        NameAttribute::Sent => "\\Sent".to_owned(),
        NameAttribute::Trash => "\\Trash".to_owned(),
        NameAttribute::Extension(other) => other.to_string(),
        _ => String::new(),
    }
}

/// How a command finished.
struct Completion {
    status: Status,
    code: Option<ResponseCode<'static>>,
    information: String,
    untagged: Vec<Response<'static>>,
}

impl Completion {
    fn ok(&self) -> bool {
        self.status == Status::Ok
    }
}

/// A parsed server response, or the raw bytes of one imap-proto rejected.
enum Frame {
    Parsed(Response<'static>),
    Raw(Vec<u8>),
}

/// Byte length of the first complete response in `buf` (lines plus literals).
fn frame_len(buf: &[u8]) -> Option<usize> {
    let mut pos = 0;
    loop {
        let newline = buf.get(pos..)?.iter().position(|&b| b == b'\n')? + pos;
        let line_end = if newline > 0 && buf[newline - 1] == b'\r' {
            newline - 1
        } else {
            newline
        };
        let line = &buf[pos..line_end];
        if line.last() == Some(&b'}')
            && let Some(open) = line.iter().rposition(|&b| b == b'{')
        {
            let digits = &line[open + 1..line.len() - 1];
            let digits = digits.strip_suffix(b"+").unwrap_or(digits);
            if !digits.is_empty() && digits.iter().all(u8::is_ascii_digit) {
                let n: usize = std::str::from_utf8(digits).ok()?.parse().ok()?;
                let literal_end = newline + 1 + n;
                if buf.len() < literal_end {
                    return None;
                }
                pos = literal_end;
                continue;
            }
        }
        return Some(newline + 1);
    }
}

/// The IMAP session over `S` (TLS in production, an in-memory duplex in tests).
pub struct RawClient<S> {
    stream: S,
    buf: Vec<u8>,
    tag: u32,
    capabilities: HashSet<String>,
    selected: Option<SelectedMailbox>,
    exists: Option<u32>,
    usable: bool,
    events: Vec<MailboxEvent>,
    io_timeout: Duration,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> RawClient<S> {
    /// Reads the server greeting.
    pub async fn greet(stream: S, io_timeout: Duration) -> ImapResult<Self> {
        let mut client = Self {
            stream,
            buf: Vec::with_capacity(16 * 1024),
            tag: 0,
            capabilities: HashSet::new(),
            selected: None,
            exists: None,
            usable: true,
            events: Vec::new(),
            io_timeout,
        };
        match client.next_frame("greeting").await? {
            Frame::Parsed(Response::Data {
                status: Status::Ok | Status::PreAuth,
                code,
                ..
            }) => {
                if let Some(ResponseCode::Capabilities(caps)) = code {
                    client.set_capabilities(&caps);
                }
                Ok(client)
            }
            Frame::Parsed(Response::Data {
                status: Status::Bye,
                information,
                ..
            }) => Err(ImapError::new(
                "connect",
                information.map_or_else(
                    || "server refused the connection".to_owned(),
                    |i| i.into_owned(),
                ),
            )),
            _ => Err(ImapError::new("connect", "unexpected server greeting")),
        }
    }

    /// LOGIN, then CAPABILITY: pre-login capabilities (minimal on iCloud) are
    /// never trusted.
    pub async fn login(&mut self, user: &str, pass: &str) -> ImapResult<()> {
        let done = self
            .run(
                "login",
                vec![text("LOGIN "), astring(user), text(" "), astring(pass)],
            )
            .await?;
        if !done.ok() {
            return Err(ImapError::new(
                "login",
                if done.information.is_empty() {
                    "authentication failed".to_owned()
                } else {
                    done.information
                },
            ));
        }
        self.refresh_capabilities().await
    }

    pub async fn refresh_capabilities(&mut self) -> ImapResult<()> {
        let done = self.run("CAPABILITY", vec![text("CAPABILITY")]).await?;
        if !done.ok() {
            return Err(ImapError::new("CAPABILITY", done.information));
        }
        let mut caps: Vec<Capability<'static>> = Vec::new();
        for response in done.untagged {
            if let Response::Capabilities(list) = response {
                caps.extend(list);
            }
        }
        self.set_capabilities(&caps);
        Ok(())
    }

    fn set_capabilities(&mut self, caps: &[Capability<'_>]) {
        self.capabilities = caps
            .iter()
            .map(|c| match c {
                Capability::Imap4rev1 => "IMAP4REV1".to_owned(),
                Capability::Auth(mechanism) => format!("AUTH={}", mechanism.to_ascii_uppercase()),
                Capability::Atom(atom) => atom.to_ascii_uppercase(),
            })
            .collect();
    }

    fn fail<T>(&mut self, operation: &str, detail: impl Into<String>) -> ImapResult<T> {
        self.usable = false;
        Err(ImapError::new(operation, detail))
    }

    async fn read_more(&mut self, operation: &str) -> ImapResult<()> {
        if self.buf.len() > MAX_RESPONSE_BYTES {
            return self.fail(operation, "server response exceeds the size limit");
        }
        self.buf.reserve(16 * 1024);
        match tokio::time::timeout(self.io_timeout, self.stream.read_buf(&mut self.buf)).await {
            Err(_) => self.fail(operation, "timed out waiting for the server"),
            Ok(Err(e)) => self.fail(operation, e.to_string()),
            Ok(Ok(0)) => self.fail(operation, "connection closed by the server"),
            Ok(Ok(_)) => Ok(()),
        }
    }

    fn take_frame(&mut self) -> Option<Frame> {
        let len = frame_len(&self.buf)?;
        let mut bytes: Vec<u8> = self.buf.drain(..len).collect();
        // imap-proto has no XLIST grammar; an XLIST row is a LIST row.
        if bytes.len() > 8 && bytes[..8].eq_ignore_ascii_case(b"* XLIST ") {
            bytes.splice(2..7, b"LIST".iter().copied());
        }
        let parsed = match imap_proto::parser::parse_response(&bytes) {
            Ok((&[], response)) => Some(response.into_owned()),
            _ => None,
        };
        Some(match parsed {
            Some(response) => Frame::Parsed(response),
            None => Frame::Raw(bytes),
        })
    }

    async fn next_frame(&mut self, operation: &str) -> ImapResult<Frame> {
        loop {
            if let Some(frame) = self.take_frame() {
                return Ok(frame);
            }
            self.read_more(operation).await?;
        }
    }

    async fn write_all(&mut self, operation: &str, bytes: &[u8]) -> ImapResult<()> {
        let result = tokio::time::timeout(self.io_timeout, async {
            self.stream.write_all(bytes).await?;
            self.stream.flush().await
        })
        .await;
        match result {
            Err(_) => self.fail(operation, "timed out writing to the server"),
            Ok(Err(e)) => self.fail(operation, e.to_string()),
            Ok(Ok(())) => Ok(()),
        }
    }

    fn next_tag(&mut self) -> String {
        self.tag += 1;
        format!("A{:04}", self.tag)
    }

    /// Tracks EXISTS/EXPUNGE/FETCH changes for the selected mailbox.
    fn observe(&mut self, response: &Response<'_>, in_select: bool, in_fetch: bool) {
        match response {
            Response::MailboxData(MailboxDatum::Exists(n)) => {
                if !in_select && self.exists.is_some_and(|prev| prev != *n) {
                    self.events.push(MailboxEvent::Exists);
                }
                self.exists = Some(*n);
            }
            Response::Expunge(_) => {
                self.exists = self.exists.map(|n| n.saturating_sub(1));
                self.events.push(MailboxEvent::Expunge);
            }
            Response::Vanished { .. } => self.events.push(MailboxEvent::Expunge),
            Response::Fetch(_, attributes)
                if !in_fetch
                    && attributes
                        .iter()
                        .any(|a| matches!(a, AttributeValue::Flags(_))) =>
            {
                self.events.push(MailboxEvent::Flags);
            }
            Response::Data {
                status: Status::Bye,
                ..
            } => self.usable = false,
            _ => {}
        }
    }

    async fn run(&mut self, operation: &str, parts: Vec<Part>) -> ImapResult<Completion> {
        self.run_with(operation, parts, false, false).await
    }

    /// Sends one tagged command and collects responses until its completion.
    async fn run_with(
        &mut self,
        operation: &str,
        parts: Vec<Part>,
        in_select: bool,
        in_fetch: bool,
    ) -> ImapResult<Completion> {
        if !self.usable {
            return Err(ImapError::new(
                operation,
                "IMAP connection is not available",
            ));
        }
        let tag = self.next_tag();
        let literal_plus = self.capabilities.contains("LITERAL+");
        let mut untagged: Vec<Response<'static>> = Vec::new();
        let mut line: Vec<u8> = format!("{tag} ").into_bytes();
        for part in parts {
            match part {
                Part::Text(bytes) => line.extend_from_slice(&bytes),
                Part::Literal(bytes) => {
                    let marker = if literal_plus {
                        format!("{{{}+}}\r\n", bytes.len())
                    } else {
                        format!("{{{}}}\r\n", bytes.len())
                    };
                    line.extend_from_slice(marker.as_bytes());
                    self.write_all(operation, &line).await?;
                    line.clear();
                    if !literal_plus {
                        // Wait for the continuation; a tagged reply ends the command early.
                        loop {
                            match self.next_frame(operation).await? {
                                Frame::Parsed(Response::Continue { .. }) => break,
                                Frame::Parsed(Response::Done {
                                    tag: done_tag,
                                    status,
                                    code,
                                    information,
                                }) if done_tag.0 == tag => {
                                    return Ok(Completion {
                                        status,
                                        code,
                                        information: information
                                            .map(Cow::into_owned)
                                            .unwrap_or_default(),
                                        untagged,
                                    });
                                }
                                Frame::Parsed(response) => {
                                    self.observe(&response, in_select, in_fetch);
                                    untagged.push(response);
                                }
                                Frame::Raw(raw) => self.raw_untagged(operation, &raw)?,
                            }
                        }
                    }
                    self.write_all(operation, &bytes).await?;
                }
            }
        }
        line.extend_from_slice(b"\r\n");
        self.write_all(operation, &line).await?;
        loop {
            match self.next_frame(operation).await? {
                Frame::Parsed(Response::Done {
                    tag: done_tag,
                    status,
                    code,
                    information,
                }) if done_tag.0 == tag => {
                    return Ok(Completion {
                        status,
                        code,
                        information: information.map(Cow::into_owned).unwrap_or_default(),
                        untagged,
                    });
                }
                Frame::Parsed(response) => {
                    self.observe(&response, in_select, in_fetch);
                    untagged.push(response);
                }
                Frame::Raw(raw) => {
                    if let Some(done) = raw_tagged(&raw, &tag) {
                        return Ok(Completion {
                            status: done.0,
                            code: None,
                            information: done.1,
                            untagged,
                        });
                    }
                    self.raw_untagged(operation, &raw)?;
                }
            }
        }
    }

    /// An untagged response imap-proto could not parse: BYE ends the session,
    /// everything else is skipped.
    fn raw_untagged(&mut self, operation: &str, raw: &[u8]) -> ImapResult<()> {
        let head: String = String::from_utf8_lossy(&raw[..raw.len().min(64)]).into_owned();
        if head.to_ascii_uppercase().starts_with("* BYE") {
            return self.fail(operation, "server closed the session");
        }
        tracing::debug!(target: LOG, "Skipping unparsed IMAP response: {}", head.trim_end());
        Ok(())
    }

    async fn select_inner(&mut self, path: &str, read_only: bool) -> ImapResult<()> {
        if let Some(selected) = &self.selected
            && selected.path == path
            && selected.read_only == read_only
        {
            return Ok(());
        }
        let operation = if read_only { "EXAMINE" } else { "SELECT" };
        self.selected = None;
        self.exists = None;
        let done = self
            .run_with(
                operation,
                vec![text(format!("{operation} ")), mailbox(path)],
                true,
                false,
            )
            .await?;
        if !done.ok() {
            return Err(ImapError::new(
                format!("{operation} {path}"),
                done.information,
            ));
        }
        let mut validity = None;
        for response in &done.untagged {
            if let Response::Data {
                code: Some(ResponseCode::UidValidity(v)),
                ..
            } = response
            {
                validity = Some(*v);
            }
        }
        let server_read_only = matches!(done.code, Some(ResponseCode::ReadOnly));
        let Some(validity) = validity else {
            return Err(ImapError::new(
                format!("{operation} {path}"),
                "no UIDVALIDITY in response",
            ));
        };
        self.selected = Some(SelectedMailbox {
            path: path.to_owned(),
            uid_validity: validity.to_string(),
            read_only: read_only || server_read_only,
        });
        Ok(())
    }

    /// imapflow `list()`: XLIST only when SPECIAL-USE is absent, `RETURN
    /// (SPECIAL-USE)` when it is present (retried without on BAD), and server
    /// role flags trusted only under one of those extensions.
    async fn list_inner(&mut self) -> ImapResult<Vec<MailboxInfo>> {
        let special_use = self.capabilities.contains("SPECIAL-USE");
        let xlist = self.capabilities.contains("XLIST") && !special_use;
        let command = if xlist { "XLIST" } else { "LIST" };
        let base = format!("{command} \"\" \"*\"");
        let mut done = if special_use {
            self.run(command, vec![text(format!("{base} RETURN (SPECIAL-USE)"))])
                .await?
        } else {
            self.run(command, vec![text(&base)]).await?
        };
        if special_use && done.status == Status::Bad {
            done = self.run(command, vec![text(&base)]).await?;
        }
        if !done.ok() {
            return Err(ImapError::new(command, done.information));
        }
        let rows = done
            .untagged
            .into_iter()
            .filter_map(|response| match response {
                Response::MailboxData(MailboxDatum::List {
                    name_attributes,
                    delimiter,
                    name,
                }) => Some(ListRow {
                    path: utf7::decode(&name),
                    flags: name_attributes
                        .iter()
                        .map(name_attribute)
                        .filter(|f| !f.is_empty())
                        .collect(),
                    delimiter: delimiter.map(Cow::into_owned),
                }),
                _ => None,
            })
            .collect();
        Ok(resolve(rows, special_use || xlist, xlist))
    }

    async fn status_inner(&mut self, path: &str) -> ImapResult<FolderStatus> {
        let done = self
            .run(
                "STATUS",
                vec![
                    text("STATUS "),
                    mailbox(path),
                    text(" (UIDNEXT UIDVALIDITY)"),
                ],
            )
            .await?;
        if !done.ok() {
            return Err(ImapError::new(format!("STATUS {path}"), done.information));
        }
        let mut status = FolderStatus::default();
        for response in &done.untagged {
            if let Response::MailboxData(MailboxDatum::Status { status: attrs, .. }) = response {
                for attr in attrs {
                    match attr {
                        StatusAttribute::UidNext(n) => status.uid_next = Some(*n),
                        StatusAttribute::UidValidity(v) => status.uid_validity = Some(*v),
                        _ => {}
                    }
                }
            }
        }
        Ok(status)
    }

    /// imapflow's search compiler. With WITHIN (RFC 5032) it sends
    /// `YOUNGER`/`OLDER` seconds relative to `now_ms` instead of day-precision
    /// `SINCE`/`BEFORE`, exactly like imapflow.
    fn search_parts(criteria: &SearchCriteria, within_now_ms: Option<i64>) -> Vec<Part> {
        let strings = [
            criteria.text.as_deref(),
            criteria.from.as_deref(),
            criteria.to.as_deref(),
            criteria.subject.as_deref(),
            criteria.header.as_ref().map(|(_, v)| v.as_str()),
        ];
        let unicode = strings.iter().flatten().any(|s| !s.is_ascii());
        let mut parts = vec![text("UID SEARCH")];
        if unicode {
            parts.push(text(" CHARSET UTF-8"));
        }
        let mut any = false;
        let push_string = |parts: &mut Vec<Part>, key: &str, value: &str| {
            parts.push(text(format!(" {key} ")));
            parts.push(if value.is_ascii() {
                astring(value)
            } else {
                Part::Literal(value.as_bytes().to_vec())
            });
        };
        if let Some(v) = &criteria.text {
            push_string(&mut parts, "TEXT", v);
            any = true;
        }
        if let Some(v) = &criteria.from {
            push_string(&mut parts, "FROM", v);
            any = true;
        }
        if let Some(v) = &criteria.to {
            push_string(&mut parts, "TO", v);
            any = true;
        }
        if let Some(v) = &criteria.subject {
            push_string(&mut parts, "SUBJECT", v);
            any = true;
        }
        if let Some(seen) = criteria.seen {
            parts.push(text(if seen { " SEEN" } else { " UNSEEN" }));
            any = true;
        }
        if let Some(since) = criteria.since_ms {
            parts.push(text(match within_now_ms {
                Some(now) => format!(" YOUNGER {}", within_seconds(now, since)),
                None => format!(" SINCE {}", format_search_date(since)),
            }));
            any = true;
        }
        if let Some(before) = criteria.before_ms
            && let Some(now) = within_now_ms
        {
            parts.push(text(format!(" OLDER {}", within_seconds(now, before))));
            any = true;
        } else if let Some(before) = criteria.before_ms {
            let midnight = omni_core::js::to_iso_string(before).ends_with("T00:00:00.000Z");
            let adjusted = if midnight {
                before
            } else {
                before + 24 * 3600 * 1000
            };
            parts.push(text(format!(" BEFORE {}", format_search_date(adjusted))));
            any = true;
        }
        if let Some((name, value)) = &criteria.header {
            parts.push(text(" HEADER "));
            parts.push(astring(name));
            parts.push(text(" "));
            parts.push(if value.is_ascii() {
                astring(value)
            } else {
                Part::Literal(value.as_bytes().to_vec())
            });
            any = true;
        }
        if !any {
            parts.push(text(" ALL"));
        }
        parts
    }

    async fn search_inner(&mut self, criteria: &SearchCriteria) -> ImapResult<Option<Vec<u32>>> {
        let within = self.capabilities.contains("WITHIN").then(system_now_ms);
        let done = self
            .run("UID SEARCH", Self::search_parts(criteria, within))
            .await?;
        if !done.ok() {
            return Ok(None);
        }
        let mut uids = Vec::new();
        for response in done.untagged {
            if let Response::MailboxData(MailboxDatum::Search(found)) = response {
                uids.extend(found);
            }
        }
        uids.sort_unstable();
        Ok(Some(uids))
    }

    async fn fetch_inner(
        &mut self,
        set: &UidSet,
        query: FetchQuery,
    ) -> ImapResult<Vec<FetchedMessage>> {
        if let UidSet::List(list) = set
            && list.is_empty()
        {
            return Ok(Vec::new());
        }
        let mut items = vec!["UID"];
        if query.flags {
            items.push("FLAGS");
        }
        if query.internal_date {
            items.push("INTERNALDATE");
        }
        if query.size {
            items.push("RFC822.SIZE");
        }
        if query.envelope {
            items.push("ENVELOPE");
        }
        let body = match query.source {
            Some(SourceRange::Full) => Some("BODY.PEEK[]".to_owned()),
            Some(SourceRange::Prefix { max_length }) => {
                Some(format!("BODY.PEEK[]<0.{max_length}>"))
            }
            None => None,
        };
        let mut list: Vec<String> = items.iter().map(|s| (*s).to_owned()).collect();
        list.extend(body);
        let command = format!("UID FETCH {} ({})", uid_set(set), list.join(" "));
        let done = self
            .run_with("UID FETCH", vec![text(command)], false, true)
            .await?;
        if !done.ok() {
            return Err(ImapError::new("UID FETCH", done.information));
        }
        let mut out = Vec::new();
        for response in done.untagged {
            let Response::Fetch(_, attributes) = response else {
                continue;
            };
            let mut message = FetchedMessage::default();
            let mut has_uid = false;
            for attribute in attributes {
                match attribute {
                    AttributeValue::Uid(uid) => {
                        message.uid = uid;
                        has_uid = true;
                    }
                    AttributeValue::Flags(flags) => {
                        message.flags = Some(flags.into_iter().map(Cow::into_owned).collect());
                    }
                    AttributeValue::InternalDate(date) => {
                        message.internal_date_ms = parse_internal_date(&date);
                    }
                    AttributeValue::Rfc822Size(size) => message.size = Some(u64::from(size)),
                    AttributeValue::Envelope(envelope) => {
                        message.envelope_message_id = envelope
                            .message_id
                            .as_ref()
                            .map(|id| String::from_utf8_lossy(id).trim().to_owned());
                    }
                    AttributeValue::BodySection { data, .. } => {
                        message.source = Some(data.map(Cow::into_owned).unwrap_or_default());
                    }
                    AttributeValue::Rfc822(data) => {
                        message.source = Some(data.map(Cow::into_owned).unwrap_or_default());
                    }
                    _ => {}
                }
            }
            if has_uid {
                out.push(message);
            }
        }
        Ok(out)
    }

    async fn copy_or_move(
        &mut self,
        verb: &str,
        uid: u32,
        destination: &str,
    ) -> ImapResult<Option<CopyUid>> {
        let operation = format!("UID {verb}");
        let done = self
            .run(
                &operation,
                vec![text(format!("UID {verb} {uid} ")), mailbox(destination)],
            )
            .await?;
        if !done.ok() {
            return Ok(None);
        }
        let mapping = done.code.as_ref().and_then(copy_uid).or_else(|| {
            done.untagged.iter().find_map(|r| match r {
                Response::Data {
                    code: Some(code), ..
                } => copy_uid(code),
                _ => None,
            })
        });
        Ok(Some(mapping.unwrap_or(CopyUid {
            uid_validity: 0,
            uid_map: Vec::new(),
        })))
    }

    async fn store_inner(
        &mut self,
        uids: &[u32],
        flags: &[&str],
        silent: bool,
    ) -> ImapResult<bool> {
        if uids.is_empty() {
            return Ok(true);
        }
        let item = if silent { "+FLAGS.SILENT" } else { "+FLAGS" };
        let command = format!(
            "UID STORE {} {item} ({})",
            uid_set(&UidSet::List(uids.to_vec())),
            flags.join(" ")
        );
        let done = self.run("UID STORE", vec![text(command)]).await?;
        Ok(done.ok())
    }

    async fn expunge_inner(&mut self, uid: u32) -> ImapResult<bool> {
        if !self.capabilities.contains("UIDPLUS") || uid == 0 {
            return Err(ImapError::new(
                "UID EXPUNGE",
                "Exact UID EXPUNGE unavailable",
            ));
        }
        let done = self
            .run("UID EXPUNGE", vec![text(format!("UID EXPUNGE {uid}"))])
            .await?;
        Ok(done.ok())
    }

    async fn append_inner(
        &mut self,
        path: &str,
        content: &[u8],
        flags: &[&str],
        internal_date_ms: Option<i64>,
    ) -> ImapResult<bool> {
        let mut parts = vec![text("APPEND "), mailbox(path)];
        if !flags.is_empty() {
            parts.push(text(format!(" ({})", flags.join(" "))));
        }
        if let Some(ms) = internal_date_ms {
            parts.push(text(format!(" \"{}\"", format_append_date(ms))));
        }
        parts.push(text(" "));
        parts.push(Part::Literal(content.to_vec()));
        let done = self.run("APPEND", parts).await?;
        Ok(done.ok())
    }

    async fn idle_inner(&mut self, interrupt: &Notify, max: Duration) -> ImapResult<IdleEnd> {
        let wake = interrupt.notified();
        tokio::pin!(wake);
        if !self.capabilities.contains("IDLE") {
            // imapflow falls back to NOOP polling without IDLE.
            let end = tokio::select! {
                () = &mut wake => IdleEnd::Interrupted,
                () = tokio::time::sleep(max) => IdleEnd::TimedOut,
            };
            let done = self.run("NOOP", vec![text("NOOP")]).await?;
            let _ = done;
            return Ok(end);
        }
        let tag = self.next_tag();
        self.write_all("IDLE", format!("{tag} IDLE\r\n").as_bytes())
            .await?;
        // Wait for the continuation.
        loop {
            match self.next_frame("IDLE").await? {
                Frame::Parsed(Response::Continue { .. }) => break,
                Frame::Parsed(Response::Done {
                    tag: t,
                    status,
                    information,
                    ..
                }) if t.0 == tag => {
                    let detail = information.map(Cow::into_owned).unwrap_or_default();
                    if status == Status::Ok {
                        return Ok(IdleEnd::TimedOut);
                    }
                    return Err(ImapError::new("IDLE", detail));
                }
                Frame::Parsed(response) => self.observe(&response, false, false),
                Frame::Raw(raw) => self.raw_untagged("IDLE", &raw)?,
            }
        }
        let deadline = tokio::time::sleep(max);
        tokio::pin!(deadline);
        let mut end = IdleEnd::TimedOut;
        'idle: loop {
            while let Some(frame) = self.take_frame() {
                match frame {
                    Frame::Parsed(response) => {
                        self.observe(&response, false, false);
                        if !self.usable {
                            return Err(ImapError::new("IDLE", "server closed the session"));
                        }
                        if !self.events.is_empty() {
                            end = IdleEnd::ServerEvent;
                            break 'idle;
                        }
                    }
                    Frame::Raw(raw) => self.raw_untagged("IDLE", &raw)?,
                }
            }
            self.buf.reserve(4096);
            tokio::select! {
                () = &mut wake => {
                    end = IdleEnd::Interrupted;
                    break 'idle;
                }
                () = &mut deadline => break 'idle,
                read = self.stream.read_buf(&mut self.buf) => match read {
                    Ok(0) => return self.fail("IDLE", "connection closed by the server"),
                    Ok(_) => {}
                    Err(e) => return self.fail("IDLE", e.to_string()),
                },
            }
        }
        self.write_all("IDLE", b"DONE\r\n").await?;
        loop {
            match self.next_frame("IDLE").await? {
                Frame::Parsed(Response::Done { tag: t, .. }) if t.0 == tag => return Ok(end),
                Frame::Parsed(response) => self.observe(&response, false, false),
                Frame::Raw(raw) => {
                    if raw_tagged(&raw, &tag).is_some() {
                        return Ok(end);
                    }
                    self.raw_untagged("IDLE", &raw)?;
                }
            }
        }
    }

    async fn logout_inner(&mut self) {
        if !self.usable {
            return;
        }
        let tag = self.next_tag();
        if self
            .write_all("LOGOUT", format!("{tag} LOGOUT\r\n").as_bytes())
            .await
            .is_err()
        {
            return;
        }
        let deadline = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                () = &mut deadline => break,
                frame = self.next_frame("LOGOUT") => match frame {
                    Ok(Frame::Parsed(Response::Done { tag: t, .. })) if t.0 == tag => break,
                    Ok(_) => {}
                    Err(_) => break,
                },
            }
        }
        self.usable = false;
        let _ = self.stream.shutdown().await;
    }
}

/// `<tag> OK|NO|BAD ...` from a raw line imap-proto could not parse.
fn raw_tagged(raw: &[u8], tag: &str) -> Option<(Status, String)> {
    let line = String::from_utf8_lossy(raw);
    let rest = line.strip_prefix(tag)?.strip_prefix(' ')?;
    let (status, information) = rest.split_once(' ').unwrap_or((rest.trim_end(), ""));
    let status = match status.trim_end().to_ascii_uppercase().as_str() {
        "OK" => Status::Ok,
        "NO" => Status::No,
        "BAD" => Status::Bad,
        _ => return None,
    };
    Some((status, information.trim_end().to_owned()))
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> ImapClient for RawClient<S> {
    fn capabilities(&self) -> &HashSet<String> {
        &self.capabilities
    }

    fn selected(&self) -> Option<&SelectedMailbox> {
        self.selected.as_ref()
    }

    fn usable(&self) -> bool {
        self.usable
    }

    fn take_events(&mut self) -> Vec<MailboxEvent> {
        std::mem::take(&mut self.events)
    }

    fn list(&mut self) -> BoxFuture<'_, ImapResult<Vec<MailboxInfo>>> {
        Box::pin(self.list_inner())
    }

    fn select<'a>(&'a mut self, path: &'a str, read_only: bool) -> BoxFuture<'a, ImapResult<()>> {
        Box::pin(self.select_inner(path, read_only))
    }

    fn status<'a>(&'a mut self, path: &'a str) -> BoxFuture<'a, ImapResult<FolderStatus>> {
        Box::pin(self.status_inner(path))
    }

    fn uid_search<'a>(
        &'a mut self,
        criteria: &'a SearchCriteria,
    ) -> BoxFuture<'a, ImapResult<Option<Vec<u32>>>> {
        Box::pin(self.search_inner(criteria))
    }

    fn uid_fetch<'a>(
        &'a mut self,
        uids: &'a UidSet,
        query: FetchQuery,
    ) -> BoxFuture<'a, ImapResult<Vec<FetchedMessage>>> {
        Box::pin(self.fetch_inner(uids, query))
    }

    fn uid_move<'a>(
        &'a mut self,
        uid: u32,
        destination: &'a str,
    ) -> BoxFuture<'a, ImapResult<Option<CopyUid>>> {
        Box::pin(self.copy_or_move("MOVE", uid, destination))
    }

    fn uid_copy<'a>(
        &'a mut self,
        uid: u32,
        destination: &'a str,
    ) -> BoxFuture<'a, ImapResult<Option<CopyUid>>> {
        Box::pin(self.copy_or_move("COPY", uid, destination))
    }

    fn uid_store_add_flags<'a>(
        &'a mut self,
        uids: &'a [u32],
        flags: &'a [&'a str],
        silent: bool,
    ) -> BoxFuture<'a, ImapResult<bool>> {
        Box::pin(self.store_inner(uids, flags, silent))
    }

    fn uid_expunge(&mut self, uid: u32) -> BoxFuture<'_, ImapResult<bool>> {
        Box::pin(self.expunge_inner(uid))
    }

    fn append<'a>(
        &'a mut self,
        path: &'a str,
        content: &'a [u8],
        flags: &'a [&'a str],
        internal_date_ms: Option<i64>,
    ) -> BoxFuture<'a, ImapResult<bool>> {
        Box::pin(self.append_inner(path, content, flags, internal_date_ms))
    }

    fn idle<'a>(
        &'a mut self,
        interrupt: &'a Notify,
        max: Duration,
    ) -> BoxFuture<'a, ImapResult<IdleEnd>> {
        Box::pin(self.idle_inner(interrupt, max))
    }

    fn logout(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(self.logout_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_lines_and_literals() {
        assert_eq!(frame_len(b"* OK hi\r\nrest"), Some(9));
        assert_eq!(frame_len(b"* 1 FETCH (BODY[] {3}\r\nabc)\r\n"), Some(29));
        assert_eq!(frame_len(b"* 1 FETCH (BODY[] {3}\r\nab"), None);
    }

    #[test]
    fn formats_dates_like_imapflow() {
        assert_eq!(format_search_date(1_788_256_800_000), "01-Sep-2026");
        assert_eq!(
            format_append_date(1_788_256_800_000),
            " 1-Sep-2026 10:00:00 +0000"
        );
        assert_eq!(
            parse_internal_date("01-Sep-2026 06:00:00 -0400"),
            Some(1_788_256_800_000)
        );
        assert_eq!(
            parse_internal_date(" 1-Sep-2026 10:00:00 +0000"),
            Some(1_788_256_800_000)
        );
    }

    #[test]
    fn compresses_uid_sets() {
        assert_eq!(uid_set(&UidSet::List(vec![5, 1, 2, 3, 9])), "1:3,5,9");
        assert_eq!(uid_set(&UidSet::From(7)), "7:*");
    }
}
