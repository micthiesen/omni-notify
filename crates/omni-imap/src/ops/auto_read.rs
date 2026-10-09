//! Auto-read cleanup (`src/email/imap/autoRead.ts`): marks unread mail from the
//! last 24 hours `\Seen` in the server-designated Archive, Junk and Trash
//! mailboxes, leaving archive-action copies unread.

use std::collections::HashSet;

use futures::future::BoxFuture;

use crate::protocol::{FetchQuery, ImapClient, ImapError, MailboxInfo, SearchCriteria, fetch_one};

const LOG: &str = "IMAP";
const AUTO_READ_AGE_MS: i64 = 24 * 60 * 60_000;
const AUTO_READ_ROLES: [&str; 3] = ["\\Archive", "\\Junk", "\\Trash"];

/// Which UIDs (or exact Message-IDs) must stay unread in a folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutoReadProtection {
    pub skip: bool,
    pub excluded_uids: HashSet<u32>,
    pub fallback_message_ids: Vec<String>,
}

/// Looks up protection for `(folder, selected UIDVALIDITY)`.
pub type ProtectionFn = dyn Fn(String, Option<String>) -> BoxFuture<'static, Result<AutoReadProtection, String>>
    + Send
    + Sync;

/// The discovered plan (rediscovered after every new connection).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutoReadPlan {
    pub folders: Vec<String>,
    pub archive_folders: Vec<String>,
}

/// Server-designated Archive, Junk and Trash, by exact special-use role.
pub fn select_auto_read_folders(mailboxes: &[MailboxInfo]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for mailbox in mailboxes {
        if mailbox.has_flag("\\Noselect") {
            continue;
        }
        let Some(role) = mailbox.special_use.as_deref() else {
            continue;
        };
        if !AUTO_READ_ROLES.contains(&role) {
            continue;
        }
        if !out.contains(&mailbox.path) {
            out.push(mailbox.path.clone());
        }
    }
    out
}

pub fn select_auto_read_archive_folders(mailboxes: &[MailboxInfo]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for mailbox in mailboxes {
        if mailbox.special_use.as_deref() == Some("\\Archive")
            && !mailbox.has_flag("\\Noselect")
            && !out.contains(&mailbox.path)
        {
            out.push(mailbox.path.clone());
        }
    }
    out
}

pub async fn discover_auto_read_plan(
    client: &mut dyn ImapClient,
) -> Result<AutoReadPlan, ImapError> {
    let mailboxes = client
        .list()
        .await
        .map_err(|e| ImapError::wrap("list mailboxes", e))?;
    Ok(AutoReadPlan {
        folders: select_auto_read_folders(&mailboxes),
        archive_folders: select_auto_read_archive_folders(&mailboxes),
    })
}

async fn mark_folder(
    client: &mut dyn ImapClient,
    folder: &str,
    since_ms: i64,
    protection: Option<&ProtectionFn>,
) -> Result<(), ImapError> {
    client
        .select(folder, false)
        .await
        .map_err(|e| ImapError::wrap(format!("lock {folder}"), e))?;
    let validity = client.selected().map(|s| s.uid_validity.clone());
    let policy = match protection {
        Some(lookup) => lookup(folder.to_owned(), validity)
            .await
            .map_err(|e| ImapError::new(format!("protect {folder}"), e))?,
        None => AutoReadProtection::default(),
    };
    if policy.skip {
        return Ok(());
    }
    let criteria = SearchCriteria {
        seen: Some(false),
        since_ms: Some(since_ms),
        ..SearchCriteria::default()
    };
    let found = client
        .uid_search(&criteria)
        .await
        .map_err(|e| ImapError::wrap(format!("search {folder}"), e))?;
    let Some(found) = found else {
        return Err(ImapError::new(
            format!("search {folder}"),
            "Unread search was not confirmed",
        ));
    };
    if found.is_empty() {
        return Ok(());
    }
    let mut excluded = policy.excluded_uids.clone();
    for message_id in &policy.fallback_message_ids {
        let operation = format!("find protected {folder} message");
        let candidates = client
            .uid_search(&SearchCriteria::message_id(message_id))
            .await
            .map_err(|e| ImapError::wrap(&operation, e))?;
        let Some(candidates) = candidates.filter(|c| c.len() <= 50) else {
            return Err(ImapError::new(
                operation,
                "Protected message lookup was not bounded and confirmed",
            ));
        };
        let verify = format!("verify protected {folder} message");
        for uid in candidates {
            let candidate = fetch_one(client, uid, FetchQuery::envelope())
                .await
                .map_err(|e| ImapError::wrap(&verify, e))?;
            let Some(candidate) = candidate else {
                return Err(ImapError::new(verify, "Candidate UID vanished"));
            };
            if candidate.envelope_message_id.as_deref() == Some(message_id.as_str()) {
                excluded.insert(uid);
            }
        }
    }
    let to_mark: Vec<u32> = found
        .into_iter()
        .filter(|uid| !excluded.contains(uid))
        .collect();
    if to_mark.is_empty() {
        return Ok(());
    }
    let marked = client
        .uid_store_add_flags(&to_mark, &["\\Seen"], true)
        .await
        .map_err(|e| ImapError::wrap(format!("mark {folder}"), e))?;
    if !marked {
        return Err(ImapError::new(
            format!("mark {folder}"),
            "Flag update was not confirmed",
        ));
    }
    Ok(())
}

/// Marks recent unread messages read, continuing when one folder fails.
pub async fn mark_recent_unread_read(
    client: &mut dyn ImapClient,
    folders: &[String],
    now_ms: i64,
    protection: Option<&ProtectionFn>,
) {
    let since = now_ms - AUTO_READ_AGE_MS;
    for folder in folders {
        if let Err(error) = mark_folder(client, folder, since, protection).await {
            tracing::warn!(
                target: LOG,
                "IMAP auto-read failed for folder \"{folder}\": {}",
                error.leaf()
            );
        }
    }
}
