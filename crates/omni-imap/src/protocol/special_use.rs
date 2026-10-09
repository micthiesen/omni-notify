//! imapflow `list()` special-use resolution: server flags first, else exact
//! and decorated localized names; one mailbox per role, ranked by source and
//! then path.
//!
//! Server role flags count only when the session advertises SPECIAL-USE or
//! XLIST, like imapflow; otherwise roles come from names. Deviation: names
//! are matched after lowercasing and removing U+200E, without NFKC
//! normalization.

use super::MailboxInfo;
use super::special_use_names::{GENERIC_TOKENS, NAMES};

const FLAGS: [&str; 7] = [
    "\\All",
    "\\Archive",
    "\\Drafts",
    "\\Flagged",
    "\\Junk",
    "\\Sent",
    "\\Trash",
];
const FLAG_SORT_ORDER: [&str; 8] = [
    "\\Inbox",
    "\\Flagged",
    "\\Sent",
    "\\Drafts",
    "\\All",
    "\\Archive",
    "\\Junk",
    "\\Trash",
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Source {
    Extension = 1,
    Name = 2,
    NameGuess = 3,
}

fn lookup(name: &str) -> Option<&'static str> {
    NAMES
        .binary_search_by(|(candidate, _)| candidate.as_bytes().cmp(name.as_bytes()))
        .ok()
        .map(|i| NAMES[i].1)
}

fn is_generic(token: &str) -> bool {
    GENERIC_TOKENS
        .binary_search_by(|candidate| candidate.as_bytes().cmp(token.as_bytes()))
        .is_ok()
}

fn classify(flags: &[String], leaf: &str, trust_flags: bool) -> Option<(&'static str, Source)> {
    if trust_flags
        && let Some(flag) = FLAGS
            .iter()
            .find(|flag| flags.iter().any(|f| f.eq_ignore_ascii_case(flag)))
    {
        return Some((flag, Source::Extension));
    }
    let name = leaf.to_lowercase().replace('\u{200e}', "");
    let name = name.trim();
    if let Some(flag) = lookup(name) {
        return Some((flag, Source::Name));
    }
    let core: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || "-_/.,()[]".contains(c))
        .filter(|token| !token.is_empty() && !is_generic(token))
        .collect();
    if core.len() == 1
        && core[0] != name
        && let Some(flag) = lookup(core[0])
    {
        return Some((flag, Source::NameGuess));
    }
    None
}

/// A raw LIST row: decoded path, flags and hierarchy delimiter.
pub(crate) struct ListRow {
    pub path: String,
    pub flags: Vec<String>,
    pub delimiter: Option<String>,
}

/// `trust_flags`: the session advertises SPECIAL-USE or XLIST. `xlist`: the
/// rows came from XLIST, whose `\Inbox` flag names a localized Inbox.
pub(crate) fn resolve(rows: Vec<ListRow>, trust_flags: bool, xlist: bool) -> Vec<MailboxInfo> {
    let mut entries: Vec<MailboxInfo> = Vec::with_capacity(rows.len());
    let mut matches: Vec<(&'static str, Source, usize)> = Vec::new();
    for row in rows {
        let mut flags = row.flags;
        if flags
            .iter()
            .any(|f| f.eq_ignore_ascii_case("\\NonExistent"))
            && !flags.iter().any(|f| f.eq_ignore_ascii_case("\\Noselect"))
        {
            flags.push("\\Noselect".to_owned());
        }
        flags.retain(|f| !f.eq_ignore_ascii_case("\\Subscribed"));
        let mut path = row.path;
        if let Some(delimiter) = row.delimiter.as_deref().filter(|d| !d.is_empty())
            && path.starts_with(delimiter)
        {
            path = path[delimiter.len()..].to_owned();
        }
        if path.eq_ignore_ascii_case("INBOX") {
            path = "INBOX".to_owned();
        }
        let index = entries.len();
        if xlist && flags.iter().any(|f| f.eq_ignore_ascii_case("\\Inbox")) {
            flags.retain(|f| !f.eq_ignore_ascii_case("\\Inbox"));
            if path != "INBOX" {
                matches.push(("\\Inbox", Source::Extension, index));
            }
        }
        let nonexistent = flags
            .iter()
            .any(|f| f.eq_ignore_ascii_case("\\NonExistent"));
        if path == "INBOX" && !nonexistent {
            matches.push(("\\Inbox", Source::Name, index));
        }
        let leaf = match row.delimiter.as_deref().filter(|d| !d.is_empty()) {
            Some(delimiter) => path.rsplit(delimiter).next().unwrap_or(&path).to_owned(),
            None => path.clone(),
        };
        if let Some((flag, source)) = classify(&flags, &leaf, trust_flags)
            && (source == Source::Extension || !nonexistent)
        {
            matches.push((flag, source, index));
        }
        entries.push(MailboxInfo {
            path,
            flags,
            special_use: None,
        });
    }
    for role in FLAG_SORT_ORDER {
        let mut candidates: Vec<&(&'static str, Source, usize)> = matches
            .iter()
            .filter(|(flag, _, _)| *flag == role)
            .collect();
        candidates.sort_by(|a, b| {
            a.1.cmp(&b.1)
                .then_with(|| omni_core::js::locale_compare(&entries[a.2].path, &entries[b.2].path))
        });
        if let Some((flag, _, index)) = candidates.first()
            && entries[*index].special_use.is_none()
        {
            entries[*index].special_use = Some((*flag).to_owned());
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, flags: &[&str]) -> ListRow {
        ListRow {
            path: path.to_owned(),
            flags: flags.iter().map(|f| (*f).to_owned()).collect(),
            delimiter: Some("/".to_owned()),
        }
    }

    #[test]
    fn resolves_flags_and_names() {
        let entries = resolve(
            vec![
                row("INBOX", &[]),
                row("Sent Messages", &["\\Sent"]),
                row("Archive", &[]),
                row("Deleted Messages", &[]),
                row("Projects/Sent to clients", &[]),
            ],
            true,
            false,
        );
        let role = |path: &str| {
            entries
                .iter()
                .find(|e| e.path == path)
                .and_then(|e| e.special_use.clone())
        };
        assert_eq!(role("INBOX").as_deref(), Some("\\Inbox"));
        assert_eq!(role("Sent Messages").as_deref(), Some("\\Sent"));
        assert_eq!(role("Archive").as_deref(), Some("\\Archive"));
        assert_eq!(role("Deleted Messages").as_deref(), Some("\\Trash"));
        assert_eq!(role("Projects/Sent to clients"), None);
    }

    fn role_of(entries: &[MailboxInfo], path: &str) -> Option<String> {
        entries
            .iter()
            .find(|e| e.path == path)
            .and_then(|e| e.special_use.clone())
    }

    #[test]
    fn ignores_role_flags_without_special_use_or_xlist() {
        let rows = || {
            vec![
                row("Sent", &[]),
                row("Sent Messages", &["\\Sent"]),
                row("Mail Archive", &["\\Archive"]),
            ]
        };
        let untrusted = resolve(rows(), false, false);
        assert_eq!(role_of(&untrusted, "Sent").as_deref(), Some("\\Sent"));
        assert_eq!(role_of(&untrusted, "Sent Messages"), None);
        assert_eq!(
            role_of(&untrusted, "Mail Archive").as_deref(),
            Some("\\Archive")
        );
        let trusted = resolve(rows(), true, false);
        assert_eq!(role_of(&trusted, "Sent"), None);
        assert_eq!(
            role_of(&trusted, "Sent Messages").as_deref(),
            Some("\\Sent")
        );
    }

    #[test]
    fn xlist_inbox_flag_names_a_localized_inbox() {
        let entries = resolve(
            vec![
                row("Posteingang", &["\\Inbox"]),
                row("Gesendet", &["\\Sent"]),
            ],
            true,
            true,
        );
        assert_eq!(role_of(&entries, "Posteingang").as_deref(), Some("\\Inbox"));
        assert!(entries[0].flags.is_empty());
        assert_eq!(role_of(&entries, "Gesendet").as_deref(), Some("\\Sent"));
    }
}
