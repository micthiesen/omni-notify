# Server iCloud Reminders

Modern CloudKit Reminders runs in Omni Notify on Boris. It does not use the Mac
EventKit CLI, Codex, CalDAV, or the existing email account configuration.

## Enable on Boris

The shipped default is disabled. Michael must add the following environment
variables to the `omni-notify` service's secret configuration in
`/home/michael/compose`, and arrange for the container to receive them through
the normal deployment workflow:

| Variable | Value |
| --- | --- |
| `ICLOUD_REMINDERS_ENABLED` | `true` to opt in; absent or anything else disables |
| `ICLOUD_REMINDERS_ACCOUNT` | Apple Account email |
| `ICLOUD_REMINDERS_PASSWORD` | Apple Account password, not an app-specific password |
| `ICLOUD_REMINDERS_STORAGE_KEY` | Independently generated 32 random bytes encoded as 64 hexadecimal characters |
| `ICLOUD_REMINDERS_PUBLIC_ORIGIN` | Exact existing HTTPS Omni origin behind Nginx Proxy Manager, with no path or trailing slash |

Generate the storage key locally with a password manager or `openssl rand -hex 32`.
Keep it with the server's protected secrets and backup it separately from the
encrypted state. Do not put secrets in source control or chat. There is no extra
Reminders admin token or login.

NPM must terminate valid HTTPS, preserve the public `Host` header, and redirect
HTTP to HTTPS. Use its existing Omni proxy host. Restrict the backend port to the
proxy/trusted network; forwarded headers alone are not trusted as authentication.
The page checks HTTPS before sending a code; the API requires the exact configured
Origin for every POST. Disable proxy request-body logging for these endpoints.
The certificate must cover that exact hostname and be trusted by the device.
A certificate for another Boris service does not cover `omni.boris`; changing
page checks cannot resolve a browser certificate warning.

An application authentication failure includes a bounded diagnostic stage and,
when available, Apple's HTTP status in the page and server log. No Apple response
body, account identifier, cookie, token, password, or code is included. An NPM
502 without this structured status is a separate proxy/application reachability
problem. Do not retry sign-in automatically while investigating either failure.

After the configured container is running:

1. Open `https://<your-existing-omni-host>/reminders`.
2. Select sign-in and approve Apple's sign-in request yourself.
3. Enter the current six-digit trusted-device code on that page. Codes are sent
   in an HTTPS request body and are never saved. A challenge expires locally
   after ten minutes and allows at most five code attempts.
4. If prompted for Advanced Data Protection web access, approve on your trusted
   Apple device and select check access. Keep ADP enabled. Apple terms must be
   reviewed in Apple's own interface. Hardware security-key authentication is
   currently reported as unsupported.
5. The page reports authenticated only after reading the Reminders zone. Confirm
   your lists through `list_reminder_lists`, then exercise a disposable reminder
   through create, edit, complete, reopen, and delete. Check date behavior on your
   Apple devices before using date mutations on important reminders.

No Apple account access is required for deployment. The implementation and tests
use mock responses; first-login compatibility and account-specific ADP behavior
must be checked by the account owner. With Michael's approval, this Apple-only
client preserves upstream's required browser User-Agent for SRP/MFA and its
service User-Agent for setup/CloudKit, including the MFA-specific Referer.

## Recovery and private state

The server checks an existing session every 15 minutes. It does not start a new
sign-in or request trusted-device consent in the background. A loss of access
pauses Reminders operations only. A durable notification reservation prevents
repeated Pushover prompts for the same unresolved incident, including uncertain
notification delivery. Successful access verification resets that incident.
Pushover uses Omni's existing `PUSHOVER_USER` and `PUSHOVER_TOKEN` settings.

Apple controls session lifetime; there is no fixed or promised 30-day expiry.
Expired sessions, transient outages, rate limits, terms, device approval, and
unsupported protocol responses have distinct public states. Interrupted local
code challenges are invalidated after a restart; start a new challenge on the page.

Session/trust tokens, cookies, and the mutation ledger live in authenticated
AES-256-GCM encrypted files under `/data/reminders-private` (host:
`/home/michael/compose/volumes/omni-notify/reminders-private`). Directory mode is
0700 and files are 0600. Writes use atomic rename and fsync. Account identity is
bound into encryption authentication. The store is separate from Omni's generic
database browsing/export. Back up the encrypted directory and encryption key;
losing either loses session trust and mutation reconciliation history.

## MCP operations

`list_reminder_lists`, `list_reminders` (also searches title and notes),
`get_reminder`, `create_reminder`, `update_reminder`, `complete_reminder`,
`reopen_reminder`, and `delete_reminder` are bounded tools under existing MCP
bearer authentication. Writes recommend Executor approval. Lists and reminders
use exact CloudKit record IDs. Reads cap snapshots at 50 pages and 10,000 records;
an incomplete snapshot is an error, not an empty result.

Mutations require an idempotency key. Updates/completion/deletion also require
the current `recordChangeTag`. Only specified fields are changed. Null clears
dates, omission preserves them. Writes check individual CloudKit errors and
perform a fresh lookup before confirming success. Delete uses Apple's soft-delete
field. An interrupted or unconfirmed operation remains reserved and is never
automatically retransmitted. Read current state and make an explicit new decision
before issuing a different operation key. Confirmed repeats return the original
confirmed result, not a claim that the reminder has remained unchanged since.

Dates are Unix milliseconds, independent of the server's local timezone. Timed
dates should be converted from the intended IANA timezone by the caller. The MCP
uses midnight UTC to represent a civil date with `allDay: true`; existing timezone
fields are preserved when not patched. Existing records are not normalized.
Titles, notes, priority (0/1/5/9), flags, dates, completion and reopening are supported.

Recurrence creation/editing is not implemented. Existing recurring records are
readable, and their mutations are rejected, including completion and deletion,
to avoid corrupting recurrence or generating incorrect next occurrences.

## Upstream attribution

Adapted from [ticaki/ioBroker.icloud](https://github.com/ticaki/ioBroker.icloud),
revision `07a91933e3f05a36d9c8918ece7f3de295aef805` (v2.1.2). Source paths:
`src/lib/index.ts`, `src/lib/auth/iCSRPAuthenticator.ts`, and
`src/lib/services/reminders.ts`; protocol context: `README_ENGLISH.md`.

Copyright (c) 2026 ticaki <github@renopoint.de>. The full MIT license is preserved
in [licenses/ioBroker.icloud-MIT.txt](licenses/ioBroker.icloud-MIT.txt) and included
in the runtime image at `/app/licenses/ioBroker.icloud-MIT.txt`. The adaptation
removes ioBroker lifecycle/logging, unredacted auth diagnostics, optimistic write
success, and the recommendation to disable ADP. It uses Omni's Effect boundaries,
private persistence, logger, notifications, MCP and frontend instead.
