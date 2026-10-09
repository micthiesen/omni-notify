//! `@zone-eu/mailsplit`'s `MessageSplitter` semantics over a complete message:
//! line-based boundary detection (a node's own boundary and its parent's),
//! header blocks, inline `message/rfc822` embedding, and body bytes with the
//! line ending before a boundary removed. Node emission order and each node's
//! parent boundary drive mailparser's part numbering.

use super::header_value::{StructuredHeader, decode_header_line, parse_header_value};
use super::mimetypes::EXTENSIONS;
use super::words::decode_words;

const MAX_HEAD_SIZE: usize = 1024 * 1024;
const MAX_CHILD_NODES: usize = 1000;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SplitError {
    #[error("Max allowed child nodes exceeded")]
    TooManyNodes,
    #[error("Max header size for a MIME node exceeded")]
    HeaderTooLarge,
}

/// One raw header line: lowercased key and the folded line as a latin1 view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderLine {
    pub key: String,
    /// The raw bytes of the (possibly folded) header line.
    pub raw: Vec<u8>,
}

impl HeaderLine {
    /// mailsplit `_decodeHeaderValue`: UTF-8 when valid, else latin1.
    pub(crate) fn text(&self) -> String {
        match std::str::from_utf8(&self.raw) {
            Ok(s) => s.to_owned(),
            Err(_) => latin1(&self.raw),
        }
    }
}

pub(crate) fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Node {
    pub parent: Option<usize>,
    pub parent_boundary: Option<Vec<u8>>,
    /// The node whose boundary `parent_boundary` is. mailparser numbers
    /// parts per boundary *object*, so two nested parts reusing one boundary
    /// string are still distinct counters.
    pub boundary_owner: Option<usize>,
    pub header_bytes: Vec<u8>,
    pub headers: Vec<HeaderLine>,
    pub parsed: bool,
    pub content_type: Option<String>,
    pub content_type_header: StructuredHeader,
    pub charset: Option<String>,
    pub disposition: Option<String>,
    pub filename: Option<String>,
    pub encoding: String,
    pub flowed: bool,
    pub del_sp: bool,
    pub multipart: Option<String>,
    pub boundary: Option<Vec<u8>>,
    pub rfc822: bool,
    pub message_node: Option<bool>,
    pub body: Vec<u8>,
    pub in_body: bool,
}

impl Node {
    fn new(parent: Option<usize>, owner: Option<usize>, nodes: &[Node]) -> Self {
        let parent_boundary = owner.and_then(|o| nodes[o].boundary.clone());
        Self {
            parent,
            boundary_owner: owner.filter(|_| parent_boundary.is_some()),
            parent_boundary,
            ..Self::default()
        }
    }

    /// First header line with `key` (mailsplit `getFirst`), decoded and trimmed.
    fn get_first(&self, key: &str) -> String {
        self.headers
            .iter()
            .find(|h| h.key == key)
            .map(|h| decode_header_line(&h.text()).1)
            .unwrap_or_default()
    }

    fn parse_headers(&mut self) {
        if self.parsed {
            return;
        }
        self.parsed = true;
        self.headers = split_header_lines(&self.header_bytes);

        let disposition = parse_header_value(&self.get_first("content-disposition"));
        let has_content_type = self.headers.iter().any(|h| h.key == "content-type");
        let content_header = if has_content_type {
            self.get_first("content-type")
        } else {
            let mut detected = None;
            if let Some(filename) = disposition.params.get("filename") {
                let ext = path_extension(filename);
                if !ext.is_empty() {
                    detected = Some(detect_mime_type(ext));
                }
            }
            detected.unwrap_or_else(|| {
                if disposition
                    .value
                    .as_deref()
                    .is_some_and(|v| v.eq_ignore_ascii_case("attachment"))
                {
                    "application/octet-stream".to_owned()
                } else {
                    "text/plain".to_owned()
                }
            })
        };
        let content_type = parse_header_value(&content_header);

        let cte = self.get_first("content-transfer-encoding");
        self.encoding = strip_paren_comment(&cte).to_lowercase().trim().to_owned();
        self.content_type = content_type
            .value
            .as_deref()
            .map(|v| v.to_lowercase().trim().to_owned())
            .filter(|v| !v.is_empty());
        self.charset = content_type
            .params
            .get("charset")
            .filter(|v| !v.is_empty())
            .cloned();
        self.disposition = disposition
            .value
            .as_deref()
            .map(|v| v.to_lowercase().trim().to_owned())
            .filter(|v| !v.is_empty())
            .map(|v| decode_words(&v));
        self.filename = disposition
            .params
            .get("filename")
            .filter(|v| !v.is_empty())
            .or_else(|| content_type.params.get("name").filter(|v| !v.is_empty()))
            .map(|v| decode_words(v));
        if content_type
            .params
            .get("format")
            .is_some_and(|f| f.to_lowercase().trim() == "flowed")
        {
            self.flowed = true;
            self.del_sp = content_type
                .params
                .get("delsp")
                .is_some_and(|d| d.to_lowercase().trim() == "yes");
        }
        self.multipart = self.content_type.as_deref().and_then(|ct| {
            let (major, minor) = ct.split_once('/')?;
            (major == "multipart" && !minor.is_empty()).then(|| minor.to_owned())
        });
        self.boundary = content_type
            .params
            .get("boundary")
            .filter(|b| !b.is_empty())
            .map(|b| b.as_bytes().to_vec());
        self.rfc822 = self.content_type.as_deref() == Some("message/rfc822");
        self.content_type_header = content_type;
    }
}

/// `str.replace(/\(.*\)/g, "")` (greedy, per line).
fn strip_paren_comment(s: &str) -> String {
    match (s.find('('), s.rfind(')')) {
        (Some(open), Some(close)) if close > open => format!("{}{}", &s[..open], &s[close + 1..]),
        _ => s.to_owned(),
    }
}

/// `path.parse(filename).ext` without the dot.
fn path_extension(filename: &str) -> &str {
    let base = filename.rsplit('/').next().unwrap_or(filename);
    match base.rfind('.') {
        Some(0) | None => "",
        Some(idx) => &base[idx + 1..],
    }
}

/// libmime `detectMimeType`.
pub(crate) fn detect_mime_type(extension_or_name: &str) -> String {
    let cleaned: String = extension_or_name
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let cleaned = cleaned.trim_start_matches('.');
    let ext = cleaned.rsplit('.').next().unwrap_or(cleaned);
    EXTENSIONS
        .binary_search_by(|(candidate, _)| candidate.as_bytes().cmp(ext.as_bytes()))
        .ok()
        .map(|idx| EXTENSIONS[idx].1.to_owned())
        .unwrap_or_else(|| "application/octet-stream".to_owned())
}

/// mailsplit `Headers._parseHeaders`.
fn split_header_lines(bytes: &[u8]) -> Vec<HeaderLine> {
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], b'\r' | b'\n') {
        end -= 1;
    }
    let block = &bytes[..end];
    let mut raw_lines: Vec<Vec<u8>> = Vec::new();
    let mut start = 0;
    for (i, &b) in block.iter().enumerate() {
        if b == b'\n' {
            let stop = if i > start && block[i - 1] == b'\r' {
                i - 1
            } else {
                i
            };
            raw_lines.push(block[start..stop].to_vec());
            start = i + 1;
        }
    }
    raw_lines.push(block[start..].to_vec());

    // Fold continuation lines into their predecessor, from the bottom up.
    let mut i = raw_lines.len();
    while i > 1 {
        i -= 1;
        if matches!(raw_lines[i].first(), Some(b' ' | b'\t')) {
            let continuation = raw_lines.remove(i);
            raw_lines[i - 1].extend_from_slice(b"\r\n");
            raw_lines[i - 1].extend_from_slice(&continuation);
        }
    }
    if raw_lines.len() == 1 && raw_lines[0].is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::with_capacity(raw_lines.len());
    for (idx, raw) in raw_lines.into_iter().enumerate() {
        let text = latin1(&raw);
        if idx == 0 {
            let lower = text.to_ascii_lowercase();
            if lower.starts_with("from ") || lower.starts_with("post ") {
                continue;
            }
        }
        let key = match text.find(':') {
            Some(colon) => text[..colon].to_lowercase().trim().to_owned(),
            None => String::new(),
        };
        lines.push(HeaderLine { key, raw });
    }
    lines
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    NextChild,
    End,
    NextSibling,
    ParentEnd,
}

fn compare_boundary(line: &[u8], startpos: usize, boundary: &[u8]) -> Option<bool> {
    if line.len() < boundary.len() + 3 + startpos || line.len() > boundary.len() + 6 + startpos {
        return None;
    }
    if line.get(startpos + 2..startpos + 2 + boundary.len())? != boundary {
        return None;
    }
    for (pos, &c) in line[boundary.len() + 2 + startpos..].iter().enumerate() {
        if pos == 0 && (c == b'\r' || c == b'\n') {
            return Some(false);
        }
        if (pos == 0 || pos == 1) && c != b'-' {
            return None;
        }
        if pos == 2 && c != b'\r' && c != b'\n' {
            return None;
        }
        if pos == 3 && c != b'\n' {
            return None;
        }
    }
    Some(true)
}

pub(crate) struct Split {
    pub nodes: Vec<Node>,
    /// Indices of nodes in the order they were emitted (mailparser `node` chunks).
    pub emitted: Vec<usize>,
}

/// Splits a complete message.
pub(crate) fn split(input: &[u8]) -> Result<Split, SplitError> {
    let mut nodes = vec![Node::new(None, None, &[])];
    let mut current = 0usize;
    let mut tree: Vec<usize> = vec![0];
    let mut emitted = Vec::new();
    let mut node_counter = 1usize;

    let mut lines: Vec<(&[u8], bool)> = Vec::new();
    let mut start = 0;
    for (i, &b) in input.iter().enumerate() {
        if b == b'\n' {
            lines.push((&input[start..=i], false));
            start = i + 1;
        }
    }
    lines.push((&input[start..], true));

    for (line, last) in lines {
        if node_counter > MAX_CHILD_NODES {
            return Err(SplitError::TooManyNodes);
        }
        if let Some(boundary) = check_boundary(&nodes[current], line) {
            if nodes[current].in_body
                && nodes[current].multipart.is_none()
                && nodes[current].parent.is_some()
            {
                strip_trailing_eol(&mut nodes[current].body);
            }
            match boundary {
                Boundary::NextChild => {
                    let node = Node::new(Some(current), Some(current), &nodes);
                    nodes.push(node);
                    current = nodes.len() - 1;
                    node_counter += 1;
                }
                Boundary::End => {}
                Boundary::NextSibling => {
                    let mut parent = nodes[current].parent;
                    if let Some(p) = parent
                        && nodes[p].content_type.as_deref() == Some("message/rfc822")
                    {
                        parent = nodes[p].parent;
                    }
                    let node = Node::new(parent, parent, &nodes);
                    nodes.push(node);
                    current = nodes.len() - 1;
                    node_counter += 1;
                }
                Boundary::ParentEnd => {
                    let node = &mut nodes[current];
                    if !node.header_bytes.is_empty() && !node.parsed {
                        node.parse_headers();
                        emitted.push(current);
                    }
                    if let Some(popped) = tree.pop() {
                        current = popped;
                    }
                    nodes[current].in_body = true;
                }
            }
            if last {
                break;
            }
            continue;
        }

        if !nodes[current].in_body {
            let node = &mut nodes[current];
            node.header_bytes.extend_from_slice(line);
            if node.header_bytes.len() > MAX_HEAD_SIZE {
                return Err(SplitError::HeaderTooLarge);
            }
            let blank = line == b"\n" || line == b"\r\n";
            if last || blank {
                node.parse_headers();
                let embed = node.rfc822
                    && (node.encoding.is_empty()
                        || matches!(node.encoding.as_str(), "7bit" | "8bit" | "binary"))
                    && node.disposition.as_deref() == Some("inline");
                emitted.push(current);
                if embed {
                    node.message_node = Some(true);
                    // The embedded root keeps numbering under the rfc822 part's parent boundary.
                    let owner = node.parent.or(Some(current));
                    let child = Node::new(Some(current), owner, &nodes);
                    nodes.push(child);
                    current = nodes.len() - 1;
                    node_counter += 1;
                } else {
                    if node.rfc822 {
                        node.message_node = Some(false);
                    }
                    node.in_body = true;
                    if node.multipart.is_some() && node.boundary.is_some() {
                        tree.push(current);
                    }
                }
            }
        } else if nodes[current].multipart.is_none() {
            nodes[current].body.extend_from_slice(line);
        }
        if last {
            break;
        }
    }
    Ok(Split { nodes, emitted })
}

fn strip_trailing_eol(body: &mut Vec<u8>) {
    if body.last() == Some(&b'\n') {
        body.pop();
        if body.last() == Some(&b'\r') {
            body.pop();
        }
    } else if body.last() == Some(&b'\r') {
        body.pop();
    }
}

fn check_boundary(node: &Node, line: &[u8]) -> Option<Boundary> {
    let mut startpos = 0;
    if matches!(line.first(), Some(b'\r' | b'\n')) {
        startpos += 1;
        if line.len() >= 2 && (line[0] == b'\r' || line[1] == b'\n') {
            startpos += 1;
        }
    }
    if line.len() < 4 || line.get(startpos) != Some(&b'-') || line.get(startpos + 1) != Some(&b'-')
    {
        return None;
    }
    if let Some(own) = &node.boundary
        && let Some(end) = compare_boundary(line, startpos, own)
    {
        return Some(if end {
            Boundary::End
        } else {
            Boundary::NextChild
        });
    }
    if let Some(parent) = &node.parent_boundary
        && let Some(end) = compare_boundary(line, startpos, parent)
    {
        return Some(if end {
            Boundary::ParentEnd
        } else {
            Boundary::NextSibling
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_nested_multiparts_and_strips_boundary_line_endings() {
        let message = b"Content-Type: multipart/mixed; boundary=\"b1\"\r\n\r\npre\r\n--b1\r\nContent-Type: text/plain\r\n\r\nhello\r\n\r\n--b1\r\nContent-Type: application/pdf\r\n\r\nPDF\r\n--b1--\r\n";
        let split = split(message).expect("split");
        assert_eq!(split.emitted.len(), 3);
        assert_eq!(split.nodes[1].body, b"hello\r\n");
        assert_eq!(split.nodes[2].body, b"PDF");
        assert_eq!(
            split.nodes[2].content_type.as_deref(),
            Some("application/pdf")
        );
    }

    #[test]
    fn detects_mime_types_like_libmime() {
        assert_eq!(detect_mime_type("pdf"), "application/pdf");
        assert_eq!(detect_mime_type("report.PDF"), "application/pdf");
        assert_eq!(detect_mime_type("unknownext"), "application/octet-stream");
    }
}
