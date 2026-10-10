//! `golden-synthesize`: derives the committed HTTP fixtures from a raw production
//! capture (`capture-golden` writes those to the gitignored [`RAW_DIR`]).
//!
//! The output keeps every route, JSON shape, enum value and edge case (nulls, empty
//! strings and arrays, optional fields, truncation markers) and replaces everything
//! else: free text, names, addresses, URLs, paths, ids and identifying numbers. It is
//! deterministic: the same capture always yields byte-identical fixtures.
//!
//! - Values of [`KEEP_KEYS`] (and [`KEEP_IN`] pairs) are code-defined enums, task
//!   names and copy, and are kept verbatim; error bodies (status 400 and above) too.
//! - Every other string is replaced by a pseudonym of the same class (email,
//!   Message-ID, URL, path, UUID, digits, compound id, free text). One map spans the
//!   whole capture, so an id keeps linking the list, detail route and file name.
//! - Arrays keep the first two elements plus the first element of every further
//!   distinct shape (keys, null-ness, number kind, kept enum values).
//! - Before anything is written, no replaced original may remain in the output.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Number, Value, json};

use crate::capture::route_slug;
use crate::repo_root;

/// Raw captures (gitignored).
pub const RAW_DIR: &str = ".local/golden-capture/http";
/// Committed synthetic fixtures.
pub const FIXTURE_DIR: &str = "crates/omni-api/tests/golden/http";

/// Keys whose string values are enums or code-defined names and copy.
const KEEP_KEYS: &[&str] = &[
    "_id",
    "admitTier",
    "artifactKey",
    "authorGender",
    "category",
    "certification",
    "effort",
    "evidence",
    "feature",
    "feedback",
    "folder",
    "genres",
    "kind",
    "level",
    "logger",
    "mediaType",
    "model",
    "op",
    "operation",
    "originCountries",
    "originalLanguage",
    "outcome",
    "pipeline",
    "platform",
    "priceStatus",
    "primaryKey",
    "queueResult",
    "range",
    "recommendedPolicy",
    "resource",
    "retrieverName",
    "role",
    "schedule",
    "scope",
    "seriesStatus",
    "service",
    "slug",
    "source",
    "state",
    "status",
    "taskName",
    "tier",
    "tool",
    "trigger",
    "type",
    "verdict",
    "version",
    "voiceName",
    "voiceProvider",
    "watchlistResult",
];

/// `(parent key, key)` pairs kept only in that position (`tasks[].name` is a task
/// name, `pets[].name` is not); the copy of data entities.
const KEEP_IN: &[(&str, &str)] = &[
    ("entities", "description"),
    ("entities", "label"),
    ("entities", "warning"),
    ("summary", "description"),
    ("summary", "label"),
    ("summary", "warning"),
    ("calendar", "autoPass"),
    ("calendar", "blocked"),
    ("parcel", "autoPass"),
    ("parcel", "blocked"),
    ("retrieverAttempts", "name"),
    ("tasks", "displayName"),
    ("tasks", "name"),
];

/// Route and id words that are structural wherever they appear.
const STRUCTURAL_WORDS: &[&str] = &["api", "audio", "both", "dgg", "pods", "tmdb", "tvdb"];

/// Numeric fields that identify a person's data (catalog ids, pet weights).
const SYNTH_NUMBER_KEYS: &[&str] = &["currentWeight", "itunesId", "tmdbId", "weight"];

/// Elements kept per array regardless of shape.
const MIN_ARRAY_ITEMS: usize = 2;

/// Data rows kept in a CSV body.
const CSV_ROWS: usize = 5;

fn keep(key: &str, parent: &str) -> bool {
    KEEP_KEYS.contains(&key) || KEEP_IN.contains(&(parent, key))
}

/// One raw capture file and its synthetic counterpart.
enum Fixture {
    Json {
        route: String,
        status: u64,
        body: Value,
    },
    Body {
        route: String,
        status: u64,
        content_type: String,
        body: String,
    },
}

/// The pseudonym state shared by every fixture of one capture.
#[derive(Default)]
pub struct Synth {
    map: BTreeMap<String, String>,
    counters: BTreeMap<&'static str, u64>,
    structural: BTreeSet<String>,
    kept: BTreeSet<String>,
    seen: BTreeSet<String>,
    replaced: BTreeSet<String>,
}

impl Synth {
    fn next(&mut self, class: &'static str) -> u64 {
        let counter = self.counters.entry(class).or_insert(0);
        *counter += 1;
        *counter
    }

    /// Records the structural values of an original body before any replacement.
    fn learn(&mut self, value: &Value, key: &str, parent: &str) {
        match value {
            Value::String(s) => {
                self.seen.insert(s.clone());
                if keep(key, parent) && !is_email(s) {
                    self.kept.insert(s.clone());
                    if !s.chars().any(char::is_whitespace) {
                        self.structural.insert(s.clone());
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.learn(item, key, parent);
                }
            }
            Value::Object(map) => {
                for (k, v) in map {
                    self.learn(v, k, key);
                }
            }
            _ => {}
        }
    }

    /// A string outside the kept positions.
    fn string(&mut self, s: &str) -> String {
        if s.chars().any(char::is_whitespace) {
            self.text(s)
        } else {
            self.token(s)
        }
    }

    fn remember(&mut self, original: &str, synthetic: String) -> String {
        if original != synthetic {
            self.replaced.insert(original.to_owned());
        }
        self.map.insert(original.to_owned(), synthetic.clone());
        synthetic
    }

    /// Free text: one sentence per distinct original, keeping truncation markers.
    fn text(&mut self, s: &str) -> String {
        if is_items_marker(s) {
            return s.to_owned();
        }
        if let Some(found) = self.map.get(s) {
            return found.clone();
        }
        let (body, marker) = split_truncation(s);
        let synthetic = if body.is_empty() {
            marker.to_owned()
        } else {
            let n = self.next("text");
            format!("Synthetic text {n}.{marker}")
        };
        self.remember(s, synthetic)
    }

    /// A whitespace-free value, replaced by a pseudonym of the same class.
    pub fn token(&mut self, t: &str) -> String {
        if t.is_empty()
            || self.structural.contains(t)
            || STRUCTURAL_WORDS.contains(&t)
            || is_timestamp(t)
            || is_items_marker(t)
        {
            return t.to_owned();
        }
        if let Some(found) = self.map.get(t) {
            return found.clone();
        }
        let synthetic = if t.bytes().all(|b| b.is_ascii_digit()) {
            let n = self.next("digits");
            format!("{n:0>width$}", width = t.len())
        } else if is_uuid(t) {
            let n = self.next("uuid");
            format!("00000000-0000-4000-8000-{n:012}")
        } else if is_message_id(t) {
            let n = self.next("message");
            format!("<message-{n}@example.com>")
        } else if is_email(t) {
            let n = self.next("email");
            format!("user{n}@example.com")
        } else if t.strip_prefix('@').is_some_and(is_domain) {
            let n = self.next("sender");
            format!("@sender{n}.example.com")
        } else if is_ipv4(t) {
            let n = self.next("ip");
            format!("192.0.2.{}", n % 254 + 1)
        } else if t.starts_with("http://") || t.starts_with("https://") {
            let n = self.next("url");
            format!("https://example.com/r/{n}{}", url_extension(t))
        } else if is_domain(t) {
            let n = self.next("domain");
            format!("site{n}.example.com")
        } else if t.starts_with('/') || t.starts_with("~/") {
            self.path(t)
        } else if t.len() > 120 || (!is_compound(t) && t.contains(BLOB_CHARS)) {
            return self.text(t);
        } else if is_compound(t) {
            let mut out = String::new();
            let mut part = String::new();
            for c in t.chars() {
                if c == ':' || c == '#' {
                    out.push_str(&self.token(&part));
                    out.push(c);
                    part.clear();
                } else {
                    part.push(c);
                }
            }
            out.push_str(&self.token(&part));
            out
        } else if let Some(at) = find_uuid(t) {
            let (head, rest) = t.split_at(at);
            let (uuid, tail) = rest.split_at(36);
            let (head, tail) = (self.affix(head), self.affix(tail));
            format!("{head}{}{tail}", self.token(uuid))
        } else {
            let n = self.next("id");
            format!("id{n}")
        };
        self.remember(t, synthetic)
    }

    /// A short literal prefix or suffix around a UUID (`PET-`, `.mp3`) stays.
    fn affix(&mut self, s: &str) -> String {
        if s.len() <= 4
            && s.bytes()
                .all(|b| b.is_ascii_alphabetic() || b"-_.".contains(&b))
        {
            s.to_owned()
        } else {
            self.token(s)
        }
    }

    /// `/a/b/c.jpg` maps each segment and keeps the extension.
    fn path(&mut self, t: &str) -> String {
        let segments: Vec<String> = t
            .split('/')
            .map(|segment| {
                if segment == "~" {
                    return segment.to_owned();
                }
                match segment.rsplit_once('.') {
                    Some((stem, ext))
                        if !stem.is_empty()
                            && (1..=4).contains(&ext.len())
                            && ext.bytes().all(|b| b.is_ascii_alphanumeric()) =>
                    {
                        format!("{}.{ext}", self.token(stem))
                    }
                    _ => self.token(segment),
                }
            })
            .collect();
        segments.join("/")
    }

    /// Ids become `1001, 1002, ...`; measurements stay small and keep int vs float.
    fn number(&mut self, n: &Number, key: &str) -> Value {
        let i = self.next("number");
        if key.ends_with("Id") {
            json!(1000 + i)
        } else if n.is_f64() {
            #[allow(clippy::cast_precision_loss)]
            let step = (i % 40) as f64;
            json!(10.125 + step * 0.25)
        } else {
            json!(10 + i % 40)
        }
    }

    /// Replaces a pruned body.
    fn value(&mut self, value: &Value, key: &str, parent: &str) -> Value {
        match value {
            Value::String(s) if keep(key, parent) && !is_email(s) => Value::String(s.clone()),
            Value::String(s) => Value::String(self.string(s)),
            Value::Number(n) if SYNTH_NUMBER_KEYS.contains(&key) => self.number(n, key),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.value(item, key, parent))
                    .collect(),
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), self.value(v, k, key)))
                    .collect::<Map<_, _>>(),
            ),
            other => other.clone(),
        }
    }

    /// Maps the id segments of a route (`/api/streamers/<id>/metrics`) with the same
    /// pseudonyms as the bodies; literal route words and numbers stay.
    fn route(&mut self, route: &str) -> String {
        let (path, query) = route
            .split_once('?')
            .map_or((route, None), |(p, q)| (p, Some(q)));
        // `/api/<collection>/<id>/...`: ids start at the fourth segment.
        let path = path
            .split('/')
            .enumerate()
            .map(|(i, segment)| {
                if i >= 3 {
                    self.route_part(segment)
                } else {
                    segment.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        match query {
            None => path,
            Some(query) => {
                let query = query
                    .split('&')
                    .map(|pair| match pair.split_once('=') {
                        Some((k, v)) => format!("{k}={}", self.route_part(v)),
                        None => pair.to_owned(),
                    })
                    .collect::<Vec<_>>()
                    .join("&");
                format!("{path}?{query}")
            }
        }
    }

    fn route_part(&mut self, part: &str) -> String {
        let decoded = percent_decode(part);
        let is_id =
            self.seen.contains(&decoded) || find_uuid(&decoded).is_some() || decoded.contains('@');
        if !is_id || decoded.bytes().all(|b| b.is_ascii_digit()) {
            return part.to_owned();
        }
        let mapped = self.string(&decoded);
        if part.contains('%') {
            omni_core::js::encode_uri_component(&mapped)
        } else {
            mapped
        }
    }

    /// Replaces a CSV body: header kept, the first [`CSV_ROWS`] rows synthesized.
    fn csv(&mut self, body: &str) -> String {
        let mut lines = body.lines();
        let mut out = String::new();
        if let Some(header) = lines.next() {
            out.push_str(header);
            out.push('\n');
        }
        for line in lines.take(CSV_ROWS) {
            let cells: Vec<String> = line
                .split(',')
                .map(|cell| match cell.parse::<f64>() {
                    Ok(_) if !is_timestamp(cell) => {
                        let decimals = cell.split_once('.').map_or(0, |(_, d)| d.len());
                        let i = self.next("csv");
                        #[allow(clippy::cast_precision_loss)]
                        let value = 10.0 + (i % 40) as f64 * 0.05;
                        format!("{value:.decimals$}")
                    }
                    _ => self.string(cell),
                })
                .collect();
            out.push_str(&cells.join(","));
            out.push('\n');
        }
        out
    }
}

/// The shape of a value, with kept enum values.
fn signature(value: &Value, key: &str, parent: &str) -> String {
    match value {
        Value::Null => "n".to_owned(),
        Value::Bool(_) => "b".to_owned(),
        Value::Number(n) if n.is_f64() => "f".to_owned(),
        Value::Number(_) => "i".to_owned(),
        Value::String(s) if keep(key, parent) => format!("s={s}"),
        Value::String(s) if s.is_empty() => "s0".to_owned(),
        Value::String(s) if is_items_marker(s) => "m".to_owned(),
        Value::String(s) if split_truncation(s).1.is_empty() => "s".to_owned(),
        Value::String(_) => "st".to_owned(),
        Value::Array(items) => {
            // Kept string lists (genres, countries) are open sets, not enums.
            let shapes: BTreeSet<String> = items
                .iter()
                .map(|item| match item {
                    Value::String(_) => signature(item, "", ""),
                    _ => signature(item, key, parent),
                })
                .collect();
            format!("[{}]", shapes.into_iter().collect::<Vec<_>>().join(","))
        }
        Value::Object(map) => {
            let fields: BTreeMap<&String, String> =
                map.iter().map(|(k, v)| (k, signature(v, k, key))).collect();
            let fields: Vec<String> = fields
                .into_iter()
                .map(|(k, s)| format!("{k}:{s}"))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
    }
}

/// Drops array elements whose shape is already represented.
fn prune(value: &Value, key: &str, parent: &str) -> Value {
    match value {
        Value::Array(items) => {
            let mut shapes = BTreeSet::new();
            let mut kept = Vec::new();
            for item in items {
                let fresh = shapes.insert(signature(item, key, parent));
                if fresh || kept.len() < MIN_ARRAY_ITEMS {
                    kept.push(prune(item, key, parent));
                }
            }
            Value::Array(kept)
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), prune(v, k, key)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

/// Characters of serialized JSON, markup or quoting: such tokens are free text.
const BLOB_CHARS: [char; 8] = ['"', '{', '}', '[', ']', '<', '>', '\\'];

/// `Task:uuid`, `Pipeline#<message-id>`, `scope:@domain`: parts joined by `:`/`#`.
fn is_compound(t: &str) -> bool {
    t.contains([':', '#'])
        && t.split([':', '#'])
            .all(|part| is_message_id(part) || !part.contains(BLOB_CHARS))
}

fn is_items_marker(s: &str) -> bool {
    s.strip_prefix("… [")
        .and_then(|rest| rest.strip_suffix(" more items]"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// `("text", "… [truncated 40 chars]")` for a truncated string.
fn split_truncation(s: &str) -> (&str, &str) {
    if let Some(at) = s.rfind("… [truncated ")
        && s.ends_with(" chars]")
    {
        return s.split_at(at);
    }
    (s, "")
}

/// ISO dates and datetimes (`2026-10-09`, `2026-10-09T14:47:00.000Z`).
fn is_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
        && b[10..]
            .iter()
            .all(|c| c.is_ascii_digit() || b"T:.Z+- ".contains(c))
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

fn find_uuid(s: &str) -> Option<usize> {
    (0..s.len().saturating_sub(35))
        .find(|&i| s.is_char_boundary(i) && s.get(i..i + 36).is_some_and(is_uuid))
}

fn is_message_id(s: &str) -> bool {
    s.starts_with('<') && s.ends_with('>') && s.contains('@')
}

fn is_email(s: &str) -> bool {
    s.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty()
            && local
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._%+-".contains(&b))
            && is_domain(domain)
    })
}

fn is_domain(s: &str) -> bool {
    s.rsplit_once('.').is_some_and(|(head, tld)| {
        !head.is_empty()
            && tld.len() >= 2
            && tld.bytes().all(|b| b.is_ascii_lowercase())
            && head
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    })
}

fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok())
}

/// `.jpg` when the URL path ends in a short extension.
fn url_extension(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or("");
    match last.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && (1..=4).contains(&ext.len())
                && ext.bytes().all(|b| b.is_ascii_alphanumeric()) =>
        {
            format!(".{ext}")
        }
        _ => String::new(),
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_capture(dir: &Path) -> Result<Vec<(String, Fixture)>> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_>>()?;
    names.sort();
    let mut fixtures = Vec::new();
    for name in names {
        let path = dir.join(&name);
        let read = || std::fs::read_to_string(&path).with_context(|| format!("reading {name}"));
        if let Some(slug) = name.strip_suffix(".meta.json") {
            let meta: Value = serde_json::from_str(&read()?)?;
            let body = std::fs::read_to_string(dir.join(format!("{slug}.body")))
                .with_context(|| format!("reading {slug}.body"))?;
            fixtures.push((
                name.clone(),
                Fixture::Body {
                    route: meta["route"].as_str().context("meta route")?.to_owned(),
                    status: meta["status"].as_u64().context("meta status")?,
                    content_type: meta["contentType"].as_str().unwrap_or("").to_owned(),
                    body,
                },
            ));
        } else if name.ends_with(".json") {
            let value: Value =
                serde_json::from_str(&read()?).with_context(|| format!("parsing {name}"))?;
            fixtures.push((
                name.clone(),
                Fixture::Json {
                    route: value["route"].as_str().context("route")?.to_owned(),
                    status: value["status"].as_u64().context("status")?,
                    body: value["body"].clone(),
                },
            ));
        } else if !name.ends_with(".body") {
            bail!("unexpected capture file {name}");
        }
    }
    if fixtures.is_empty() {
        bail!("no captures in {}", dir.display());
    }
    Ok(fixtures)
}

/// Synthesizes every capture in `raw` into `{file name: contents}`.
pub fn synthesize(raw: &Path) -> Result<BTreeMap<String, String>> {
    let fixtures = read_capture(raw)?;
    let mut synth = Synth::default();
    for (_, fixture) in &fixtures {
        if let Fixture::Json { body, .. } = fixture {
            synth.learn(body, "", "");
        }
    }
    // Routes first, so file names do not shift when body pruning changes.
    let routes: Vec<String> = fixtures
        .iter()
        .map(|(_, fixture)| match fixture {
            Fixture::Json { route, .. } | Fixture::Body { route, .. } => synth.route(route),
        })
        .collect();
    let mut out = BTreeMap::new();
    for ((name, fixture), route) in fixtures.iter().zip(routes) {
        match fixture {
            Fixture::Json { status, body, .. } => {
                let body = if *status >= 400 {
                    body.clone()
                } else {
                    synth.value(&prune(body, "", ""), "", "")
                };
                let file = json!({"route": route, "status": status, "body": body});
                insert(&mut out, format!("{}.json", route_slug(&route)), &file)?;
            }
            Fixture::Body {
                status,
                content_type,
                body,
                ..
            } => {
                if !content_type.starts_with("text/csv") {
                    bail!("{name}: no synthesizer for content type {content_type:?}");
                }
                let body = synth.csv(body);
                let slug = route_slug(&route);
                let meta = json!({"route": route, "status": status, "contentType": content_type});
                insert(&mut out, format!("{slug}.meta.json"), &meta)?;
                out.insert(format!("{slug}.body"), body);
            }
        }
    }
    check_no_originals(&synth, &out)?;
    Ok(out)
}

fn insert(out: &mut BTreeMap<String, String>, name: String, value: &Value) -> Result<()> {
    let text = format!("{}\n", omni_core::js::json_stringify_pretty2(value));
    if out.insert(name.clone(), text).is_some() {
        bail!("two captures synthesize to {name}");
    }
    Ok(())
}

/// No replaced original (six or more characters with a letter, and not part of a
/// kept value) may survive anywhere in the output, file names included.
fn check_no_originals(synth: &Synth, out: &BTreeMap<String, String>) -> Result<()> {
    let haystack: String = out
        .iter()
        .flat_map(|(name, text)| [name.as_str(), "\n", text.as_str(), "\n"])
        .collect();
    let kept: Vec<&String> = synth.kept.iter().chain(&synth.structural).collect();
    let leaks: Vec<&String> = synth
        .replaced
        .iter()
        .filter(|original| {
            original.chars().count() >= 6
                && original.chars().any(char::is_alphabetic)
                && !kept.iter().any(|k| k.contains(original.as_str()))
                && (haystack.contains(original.as_str())
                    || haystack.contains(&encode_json(original)))
        })
        .collect();
    if !leaks.is_empty() {
        bail!(
            "{} replaced value(s) still appear in the synthesized output: {:?}",
            leaks.len(),
            leaks
                .iter()
                .take(5)
                .map(|l| l.chars().take(40).collect::<String>())
                .collect::<Vec<_>>()
        );
    }
    Ok(())
}

/// The JSON-escaped form of `s` without quotes.
fn encode_json(s: &str) -> String {
    let quoted = omni_core::js::json_stringify(&Value::String(s.to_owned()));
    quoted[1..quoted.len() - 1].to_owned()
}

/// `cargo xtask golden-synthesize [--from DIR] [--check]`.
pub fn golden_synthesize(args: &[String]) -> Result<()> {
    let root = repo_root();
    let raw = crate::flag_value(args, "--from").map_or_else(|| root.join(RAW_DIR), Into::into);
    let check = args.iter().any(|a| a == "--check");
    let fixtures = synthesize(&raw)?;
    let dir = root.join(FIXTURE_DIR);
    let committed = read_fixture_dir(&dir)?;
    if check {
        if committed != fixtures {
            bail!(
                "{FIXTURE_DIR} differs from the synthesized capture; run `cargo xtask golden-synthesize`"
            );
        }
        println!("golden-synthesize: ok ({} files)", fixtures.len());
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    for name in committed.keys() {
        if !fixtures.contains_key(name) {
            std::fs::remove_file(dir.join(name))?;
        }
    }
    for (name, text) in &fixtures {
        std::fs::write(dir.join(name), text).with_context(|| format!("writing {name}"))?;
    }
    println!("synthesized {} files into {FIXTURE_DIR}", fixtures.len());
    Ok(())
}

fn read_fixture_dir(dir: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    if !dir.exists() {
        return Ok(files);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".json") || name.ends_with(".body") {
            files.insert(name, std::fs::read_to_string(entry.path())?);
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_with(structural: &[&str]) -> Synth {
        Synth {
            structural: structural.iter().map(|s| (*s).to_owned()).collect(),
            ..Synth::default()
        }
    }

    #[test]
    fn tokens_keep_their_class_and_stay_consistent() {
        let mut s = synth_with(&["LiveCheckTask", "CalendarEvents", "both"]);
        assert_eq!(s.token("someone@gmail.com"), "user1@example.com");
        assert_eq!(s.token("someone@gmail.com"), "user1@example.com");
        assert_eq!(s.token("<abc/def@github.com>"), "<message-1@example.com>");
        assert_eq!(
            s.token("CalendarEvents#<abc/def@github.com>"),
            "CalendarEvents#<message-1@example.com>"
        );
        assert_eq!(s.token("both:@uber.com"), "both:@sender1.example.com");
        assert_eq!(
            s.token("LiveCheckTask:a05dc9c0-4110-4268-bbb9-1ac83212876f"),
            "LiveCheckTask:00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(
            s.token("PET-a05dc9c0-4110-4268-bbb9-1ac83212876f"),
            "PET-00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(s.token("10.10.1.100"), "192.0.2.2");
        assert_eq!(
            s.token("https://plex.boris/web/a.jpg?x=1"),
            "https://example.com/r/1.jpg"
        );
        assert_eq!(s.token("/Users/someone/poster.jpg"), "/id1/id2/id3.jpg");
        assert_eq!(
            s.token("2026-10-09T14:47:00.000Z"),
            "2026-10-09T14:47:00.000Z"
        );
        assert_eq!(s.token("814889"), "000001");
    }

    #[test]
    fn text_keeps_truncation_and_item_markers() {
        let mut s = Synth::default();
        assert_eq!(
            s.string("a private note… [truncated 40 chars]"),
            "Synthetic text 1.… [truncated 40 chars]"
        );
        assert_eq!(s.string("… [13 more items]"), "… [13 more items]");
        assert_eq!(s.string(""), "");
    }

    #[test]
    fn values_keep_enums_nulls_and_shapes() {
        let mut s = Synth::default();
        let body = json!({"tasks": [{"name": "PressPods", "status": "success", "error": null}],
            "pets": [{"name": "Rex", "weight": 12.5, "tmdbId": 7}], "items": []});
        s.learn(&body, "", "");
        assert_eq!(
            s.value(&body, "", ""),
            json!({"tasks": [{"name": "PressPods", "status": "success", "error": null}],
                "pets": [{"name": "id1", "weight": 10.375, "tmdbId": 1002}], "items": []})
        );
    }

    #[test]
    fn kept_positions_never_keep_email_addresses() {
        let mut s = Synth::default();
        let body = json!({"parcel": {"blocked": ["@ups.com", "pkginfo@ups.com"]}});
        s.learn(&body, "", "");
        assert_eq!(
            s.value(&body, "", ""),
            json!({"parcel": {"blocked": ["@ups.com", "user1@example.com"]}})
        );
    }

    #[test]
    fn prune_keeps_one_element_per_shape() {
        let items = json!([
            {"status": "ok", "x": "a"}, {"status": "ok", "x": "b"}, {"status": "ok", "x": "c"},
            {"status": "error", "x": "d"}, {"status": "ok", "x": null}
        ]);
        let pruned = prune(&items, "calls", "");
        assert_eq!(pruned.as_array().unwrap().len(), 4);
    }

    #[test]
    fn routes_map_ids_and_keep_literals() {
        let mut s = Synth::default();
        s.learn(&json!({"id": "darius"}), "", "");
        assert_eq!(
            s.route("/api/streamers/darius/metrics"),
            "/api/streamers/id1/metrics"
        );
        assert_eq!(
            s.route("/api/streamers/nobody/metrics"),
            "/api/streamers/nobody/metrics"
        );
        assert_eq!(s.route("/api/costs?days=7"), "/api/costs?days=7");
        assert_eq!(
            s.route("/api/email-activity/X%23%3Ca%40b.com%3E/logs"),
            "/api/email-activity/id2%23%3Cmessage-1%40example.com%3E/logs"
        );
    }
}
