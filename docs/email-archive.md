# Queued reversible email archive

The MCP tools `email_archive_queue`, `email_archive_status`,
`email_archive_cancel`, and `email_archive_restore` move selected Inbox messages
to the server-designated Archive and retain a durable receipt for reversal.
There is no arbitrary mailbox move, delete or purge tool.

## Select an exact message

Use `email_search` or `email_get` with `fresh: true`. Keep the exact `messageId`
and returned `origin` coordinates: `folder`, `uidValidity`, and `uid`.
Only an origin of `INBOX` is eligible. Supply these fields with an
`idempotencyKey` to `email_archive_queue`. Queue acceptance is not proof of a
completed archive; inspect `email_archive_status` using the returned action ID.

The worker runs every 30 seconds and on transport startup. Cancellation is
available while the action remains queued. A durable claim prevents two workers
from moving the same selected mail. Reusing an idempotency key for different
mail is rejected.

## Safety and recovery

Before moving, Omni selects Inbox and verifies UIDVALIDITY, the exact UID and
Message-ID. It records a SHA-256 hash of the original MIME and its flags, without
persisting another body copy. It discovers the unique `\\Archive` special-use
mailbox through IMAP LIST. A native post-login `MOVE` capability is mandatory.
ImapFlow's COPY plus delete fallback is never used.

The worker reserves the mutation durably before issuing MOVE. It verifies the
resulting destination and content/flags before reporting `archived`. A timeout,
disconnection, missing MOVE response, or crash after reservation triggers
read-only reconciliation. Omni never automatically repeats an uncertain MOVE.
Ambiguous duplicates remain uncertain rather than being selected by Message-ID
alone. UIDVALIDITY changes also prevent trusting stale coordinates.

Durable statuses distinguish queued, cancelled, claimed, archived, uncertain,
failed, restore_claimed, restored, and restore_uncertain. Status exposes safe
reason codes and attempt counts. Retrying a status read may reconcile an
uncertain result, but cannot create another move.

Restore is limited to the exact Archive destination recorded by that action.
It verifies the content again and moves it back to Inbox, retaining a second
receipt. It cannot select an arbitrary Archive message or destination. A
reserved restore with an uncertain result also reconciles without repeating the
mutation.

Native MOVE retains message content and flags. Omni's auto-read policy excludes
the exact messages owned by these archive actions, so unread mail stays unread.
Ordinary Archive, Junk and Trash cleanup continues. Restore preserves the flags
observed immediately before restoration.
Archive and restore reserve a Message-ID event suppression marker so their new
IMAP UIDs do not wake the email subscription again.

## Authorization and testing

Queue and restore require the user's selected-message authorization. Tool
availability and an event notification do not authorize archiving unrelated
mail. Drafting an automation must identify its intended scope first.

Automated tests use fabricated IMAP records and temporary durable storage. They
cover native MOVE requirements, exact identity mismatches, content/flags,
reservation and reconciliation, duplicate requests, cancellation and restoration.
No production user mail should be archived as a test without explicit selection.

Rollback is safe for stored records: removing the tools and worker leaves mail
and action receipts intact. Reverting code does not itself restore already
archived mail. Complete any desired restore using its receipt before disabling
the feature, or move the identified message back with a mail client.
