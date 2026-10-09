//! The committed golden fixtures are public: they must come from
//! `cargo xtask golden-synthesize`, never from a raw production capture. This
//! fails on obvious personal markers: email addresses and URLs outside the
//! reserved example domains, routable IPv4 addresses, non-synthetic UUIDs, and
//! known personal names, hosts and paths.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};

const EXAMPLE_DOMAINS: &[&str] = &["example.com", "example.org", "example.net"];

/// Case-insensitive markers of the owner's identity, hosts and home paths.
const MARKERS: &[&str] = &[
    "/users/",
    "/home/",
    "michael",
    "thiesen",
    ".boris",
    "boris/",
    "iodinecafe",
    "bearer ",
    "maxbook",
];

fn golden_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            golden_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn is_example_domain(domain: &str) -> bool {
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    EXAMPLE_DOMAINS
        .iter()
        .any(|d| domain == *d || domain.ends_with(&format!(".{d}")))
}

fn is_local_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "._%+-".contains(c)
}

fn is_domain_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '-'
}

/// Domains of `user@domain` addresses (`@ups.com` sender patterns have no local
/// part and are not addresses).
fn email_domains(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (at, _) in text.match_indices('@') {
        let local = text[..at]
            .chars()
            .rev()
            .take_while(|c| is_local_char(*c))
            .count();
        let domain: String = text[at + 1..]
            .chars()
            .take_while(|c| is_domain_char(*c))
            .collect();
        // `no-reply@accounts.` is a sender-prefix pattern, not an address.
        let domain = domain.trim_end_matches('.');
        if local > 0 && domain.contains('.') {
            found.push(domain.to_owned());
        }
    }
    found
}

fn url_hosts(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for scheme in ["http://", "https://"] {
        for (start, _) in text.match_indices(scheme) {
            let host: String = text[start + scheme.len()..]
                .chars()
                .take_while(|c| is_domain_char(*c))
                .collect();
            found.push(host);
        }
    }
    found
}

/// Dotted quads outside the documentation and loopback ranges.
fn routable_ipv4(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for token in text.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
        let parts: Vec<&str> = token.split('.').collect();
        let is_quad = parts.len() == 4
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok());
        let reserved = ["192.0.2.", "198.51.100.", "203.0.113.", "127.", "0."]
            .iter()
            .any(|prefix| token.starts_with(prefix));
        if is_quad && !reserved {
            found.push(token.to_owned());
        }
    }
    found
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// UUIDs that are not `00000000-0000-4000-8000-<counter>` pseudonyms.
fn real_uuids(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    (0..bytes.len().saturating_sub(35))
        .filter_map(|i| text.get(i..i + 36))
        .filter(|candidate| is_uuid(candidate))
        .filter(|uuid| !uuid.starts_with("00000000-0000-4000-8000-"))
        .map(str::to_owned)
        .collect()
}

fn findings(name: &str, text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for domain in email_domains(text) {
        if !is_example_domain(&domain) {
            problems.push(format!("email address at {domain}"));
        }
    }
    for host in url_hosts(text) {
        if !is_example_domain(&host) {
            problems.push(format!("URL host {host}"));
        }
    }
    for ip in routable_ipv4(text) {
        problems.push(format!("IPv4 address {ip}"));
    }
    for uuid in real_uuids(text) {
        problems.push(format!("non-synthetic UUID {uuid}"));
    }
    let lower = text.to_lowercase();
    for marker in MARKERS {
        if lower.contains(marker) {
            problems.push(format!("marker {marker:?}"));
        }
    }
    problems
        .into_iter()
        .map(|problem| format!("{name}: {problem}"))
        .collect()
}

#[test]
fn committed_fixtures_contain_no_personal_markers() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut files = Vec::new();
    golden_files(&root, &mut files);
    assert!(
        !files.is_empty(),
        "no golden fixtures under {}",
        root.display()
    );
    let mut problems = Vec::new();
    for path in files {
        let name = path.strip_prefix(&root).unwrap().display().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        problems.extend(findings(&name, &format!("{name}\n{text}")));
    }
    assert!(
        problems.is_empty(),
        "golden fixtures look like a raw capture; regenerate them with `cargo xtask golden-synthesize`:\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_scanner_flags_raw_capture_markers() {
    let raw = r#"{"from": "someone@gmail.com", "url": "http://plex.lan/x",
        "ip": "10.10.1.100", "id": "a05dc9c0-4110-4268-bbb9-1ac83212876f",
        "cwd": "/Users/someone/Code"}"#;
    let problems = findings("raw.json", raw);
    for expected in [
        "email address at gmail.com",
        "URL host plex.lan",
        "IPv4 address 10.10.1.100",
        "non-synthetic UUID a05dc9c0-4110-4268-bbb9-1ac83212876f",
        "marker \"/users/\"",
    ] {
        assert!(
            problems.iter().any(|p| p.ends_with(expected)),
            "{expected} not flagged in {problems:?}"
        );
    }
    let synthetic = r#"{"from": "user1@example.com", "pattern": "@ups.com",
        "url": "https://example.com/r/1.jpg", "ip": "192.0.2.4",
        "id": "PET-00000000-0000-4000-8000-000000000001", "version": "1.2.3"}"#;
    assert_eq!(findings("synthetic.json", synthetic), Vec::<String>::new());
}
