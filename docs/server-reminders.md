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

### Boris routing

Use `https://omni.syas.ca/reminders` on the home LAN. Its unproxied DNS A record
points to `10.10.1.100`. NPM manages a matching Let's Encrypt certificate through
Cloudflare DNS validation, including renewal. No public application access is
needed for certificate issuance.

Both the new hostname and the legacy `omni.boris` proxy allow `10.10.1.0/24`
and deny other TCP peers. Their server-level real-IP configuration trusts only
Unix sockets, overriding inherited CDN/private-network header trust. Forwarded
headers cannot turn an external TCP peer into an allowed LAN client. The legacy
`.boris` HTTPS certificate still does not match; use the new hostname for HTTPS.

Keep `pods.syas.ca`'s existing public `/pods/*` routes and its deny filter for other
paths. Do not apply the LAN ACL to those podcast integrations. Preserve Executor's
existing authenticated Omni connection and verify it after proxy changes.

The manual **Verify Reminders LAN boundary** GitHub workflow probes the existing
public ingress from a hosted runner, including forged LAN headers, legacy aliases,
and the podcast exceptions. Supply Boris's current public IPv4 as `origin_ipv4`.
It sends no credentials, discards response bodies, and never bypasses TLS errors.

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

`list_reminder_lists.items[].count` counts incomplete, nondeleted reminders in
that exact list, derived from the complete reminder snapshot. Apple's optional
list `Count` metadata is not used because it may be absent or stale. Counts are
recomputed after incremental changes, including completion, reopening, moves,
and deletion. The response `total` is the number of lists before pagination.
`list_reminders.total` is the number of nondeleted reminders matching all supplied
filters before pagination; omitting `completed` includes both completion states.
For an unchanged account, summing all list counts equals `list_reminders` with
`completed: false`. Separate requests can observe intervening account changes.

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

The device-popup request uses `PUT /appleauth/auth/verify/trusteddevice/securitycode`.
The parent path used by ioBroker returns HTTP 405. This endpoint correction follows
[rclone's deployed notification request](https://github.com/rclone/rclone/blob/0b8e9c4ccdec2b9b4c4f6f0b446912eb888d0a4e/backend/iclouddrive/api/session.go#L647).
Code verification uses `POST` to the same path and retains the challenge headers
from the delivery response. No popup is requested by background health checks.

## Shared protected iCloud access

`src/icloud/protectedAccess.ts` provides the service-independent PCS workflow for
Reminders and future iCloud integrations. Its caller supplies an authenticated,
cookie-persisting request adapter and serializes requests for the account. It
does not own credentials, change Apple settings, or approve device prompts.
Only an explicit user action starts the workflow. It requests service access
once, then polls at five-second intervals with `derivedFromUserAction: false`,
up to ten requests. Pending approval returns to the UI without claiming access.

Apple releases service keys to its servers through the approved session's PCS
cookie; Omni does not extract device keys. A CloudKit field can retain the
`ENCRYPTED_BYTES` type after its contents have been decrypted. Reminders accepts
only a valid bounded CRDT document, including zlib, gzip and raw protobuf forms.
Ciphertext remains blocked. The final access check decodes current protected
Reminders content, independently of complete list discovery.

The PCS workflow is adapted from MIT-licensed
[timlaing/pyicloud PR 317](https://github.com/timlaing/pyicloud/pull/317), revision
`b5f2e2a7f9e5cd5be7e009626c4ae021d8b2bb34`, `pyicloud/base.py`.
Copyright (c) 2025 The PyiCloud Authors. Its full license is preserved in
[licenses/pyicloud-MIT.txt](licenses/pyicloud-MIT.txt) and shipped in the image.

Current-record queries follow `pyicloud/services/reminders/_reads.py` at revision
`86c4bc90d5632bcaf7507e5335e127f7450a177a`, under the same MIT license.
List discovery exhausts Apple's paginated zone history before exposing results;
the client retains the completed list index and cursor for incremental refreshes.
Reminders use each list's compound query, including recurrence relationships.
Incomplete pagination or malformed recurrence records fail closed.

The initial complete snapshot runs in the application's Effect scope after access
is verified. Large accounts can take several minutes; tools return a bounded
"synchronizing" error during that initial load instead of waiting past the MCP
transport deadline. The index stays in server memory and subsequent reads apply
CloudKit changes from its completed cursor. Failed or interrupted refreshes never
publish a partial index. A container restart rebuilds it without another sign-in.
