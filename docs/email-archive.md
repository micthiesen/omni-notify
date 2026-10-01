# Queued reversible email archive

`email_archive_queue` accepts a selected Inbox `messageId` and exact `origin`
(`folder`, `uidValidity`, `uid`) from fresh `email_get` or `email_search` output.
`email_archive_status` returns its durable action ID, phase, destination, and
safe reason code. A completed restore also returns `restoredLocation`, the exact
new Inbox UID and UIDVALIDITY. `email_archive_cancel` works only before the
first mutation claim. `email_archive_restore` returns only that action's
recorded Archive copy
to Inbox. These tools cannot move arbitrary mailboxes or UIDs.

## iCloud transport and source removal

The deployed iCloud server advertised IMAP4rev1 and UIDPLUS, but neither MOVE
nor IMAP4rev2 on 2026-10-01. Omni uses native MOVE when a server advertises it.
Otherwise the UIDPLUS path copies the exact selected message to the unique
`\Archive` special-use mailbox, verifies its Message-ID, MIME hash, UIDVALIDITY,
and flags, then marks the **exact Inbox source UID** `\Deleted` and sends
`UID EXPUNGE <that UID>`. UID EXPUNGE permanently removes that source. It never
issues plain mailbox-wide EXPUNGE or calls ImapFlow's combined `messageDelete`
or MOVE emulation. Restore applies the same verified stages in reverse, using
the recorded Archive UID as its only eligible source.

The protocol distinction follows [RFC 4315](https://www.rfc-editor.org/rfc/rfc4315.html)
for COPYUID and UID EXPUNGE, and [RFC 6851](https://www.rfc-editor.org/rfc/rfc6851.html)
for native MOVE. The adapter was checked against deployed ImapFlow 1.6.5.

The eight archive actions that previously failed with `native_move_unavailable`
remain terminal. This change does not requeue them or act on live mail. A new
queue request needs a fresh selected identity and idempotency key.

## Durable phases and recovery

Before any mutation, Omni checks exact source UIDVALIDITY, Message-ID, MIME
hash, flags, and the destination for an existing identical copy. It rejects an
already `\Deleted` source. The action records a claim before each COPY, STORE,
or UID EXPUNGE. A copied message is verified against the COPYUID mapping or a
single exact mailbox match before source removal. The destination and source
are rechecked before STORE and EXPUNGE. Archive messages owned by an action
are excluded from Omni's ordinary auto-read pass so unread flags remain intact.

The phases `copy_claimed`, `copy_verified`, `delete_claimed`, and
`expunge_claimed` expose progress. Their `restore_` counterparts apply to
reversal. After a lost response or restart, the worker reads mailbox state;
it never repeats a claimed COPY, STORE, or UID EXPUNGE blindly. A verified
earlier phase may advance to the next distinct operation. Status polling only
reads mail and action records; it never starts a mailbox mutation.

`copied_source_retained` means a verified destination copy exists but the
source remains, so the action is not complete. Its reason distinguishes an
unmarked source (`copied_source_retained`) from a source still marked
`\Deleted` (`copied_source_deleted`). A `copy_claimed` action with
`copy_uncertain` may have copied mail, but Omni cannot yet identify a unique
verified destination. Inspect both mailboxes before any manual intervention.
Retained-source receipts are rechecked on status reads and worker sweeps. A
delayed STORE can update their reason; a delayed UID EXPUNGE can confirm
completion. These checks never start another mutation. Other uncertain phases
keep their exact reservation and remain read-only on recovery when the result
cannot be proven. No action reports `archived` or `restored` until the exact
destination exists and the exact source is absent.

An archive and its restore reserve the Message-ID against duplicate actions.
The UIDPLUS path suppresses event echoes only at recorded destination UIDs.
Before a destination UID is known, matching Message-ID alone does not suppress
another mailbox event. The original Inbox event remains eligible. Ordinary
Archive, Junk, and Trash auto-read policy continues for unrelated messages.

## Authorization, testing, and rollback

Queue and restore apply only to user-selected mail. A tool receipt or event
notification is not a selection of other mail. Automated tests use fabricated
IMAP messages and temporary durable storage. Do not archive real user mail as
a test without its specific selection.

A rollback to a build from before UIDPLUS action statuses is safe only before
the first UIDPLUS receipt is written. That older schema cannot decode the new
statuses and reasons. After a receipt exists, disable new mutations with a
patch to a schema-capable build while preserving receipts. A code rollback
cannot undo a completed UID EXPUNGE. Restore a completed action through its
receipt before disabling the feature if its original mailbox placement must be
recovered. A retained or uncertain action needs mailbox inspection before
further mutation.
