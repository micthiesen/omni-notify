# MCP Events through Executor

Omni implements MCP Events at its authenticated `/mcp` endpoint. The separate
`executor-events-adapter` sidecar (the Rust crate
[`omni-events-adapter`](../crates/omni-events-adapter/src/lib.rs), deployed as described in
[`deploy/executor-events/`](../deploy/executor-events/README.md)) adds event discovery to the existing Executor connection while retaining its
OAuth authority and tool catalog. It does not modify third-party Executor core.

## Verified protocol and deployment baseline

The deployment inspected on 2026-10-01 runs Executor 1.6.10, revision
`3890d6f5e5efd1530f0dba0fe23ada95a39caf86`, image digest
`sha256:b9e001775d3eb7d662d347c8f7054333c78c1a4fd97cb5a86dc1d1d1406093b9`.
It uses MCP 1.x and rejects `server/discover`. A freshly authenticated legacy
session at `/mcp?elicitation_mode=native` listed Executor's six tools and no
`resume` tool. Previously cached connection metadata is not evidence of that
session's mode.

On 2026-10-05 an unauthenticated modern `server/discover` to
`https://mcp.syas.ca/mcp` received the adapter's own 401, while a legacy
`initialize` received Executor's, so the exact `/mcp` route is live. Executor
also serves `/<org>/mcp` and `/mcp/toolkits/<slug>`; the route does not cover
those paths, so a ChatGPT connection must use exactly `/mcp` with Executor OAuth
(the adapter does not accept Executor API keys) to discover events.

Omni serves both the legacy `2025-11-25` era and the modern `2026-07-28` era
(`crates/omni-mcp/src/rpc.rs`). The adapter translates modern MCP requests to the deployed
legacy Executor host and forwards authenticated event RPC to Omni. Neither
Executor search/invoke passthrough nor ordinary SSE notifications implement
MCP Events. The required modern protocol is `2026-07-28`.

References:

- [OpenAI MCP Events](https://developers.openai.com/plugins/build/mcp-events)
- [Executor host envelope reference](https://github.com/UsefulSoftwareCo/executor/blob/98d606bd2b47b9dcc2c03a129a14b5134d9852c8/packages/hosts/mcp/src/envelope.ts)
- [Executor API reference](https://github.com/UsefulSoftwareCo/executor/blob/98d606bd2b47b9dcc2c03a129a14b5134d9852c8/packages/core/api/src/server/executor-app.ts)
- [Executor v2 contracts](https://github.com/UsefulSoftwareCo/executor/tree/2eb2871ecaaf168928eeb77fee89bfdc8e25c614)

## Events

`events/list` serves the catalog in `crates/omni-mcp/src/events/catalog.rs`. Each event
declares its arguments, payload schema and the rule that matches a payload to a
subscription. All events share one durable outbox, signing, authorization and
retry path. Neither event has a protocol replay cursor; subscription responses
use `cursor: null`. Every event also has ordinary tools for clients without MCP
Events, so subscriptions only remove polling.

### `email.received`

Takes an explicit `folder` argument of `inbox` or `archive`. Subscribe
separately for each desired scope. The event contains a Message-ID, originating
folder, UIDVALIDITY and UID. Read the full message with the existing `email_get`
tool. Subjects, bodies, attachments, and unsubscribe URLs do not travel in
callbacks. Messages without a Message-ID are excluded. Origin comes from the
actual IMAP fetch, not an inferred display name or attachment.

The first observed folder is retained across replay. Moving the same Message-ID
from Inbox to Archive does not create a second receipt event. Queued archive
and restore operations also reserve a suppression marker before moving mail.
This covers messages that predate event observation and prevents automation
feedback loops.

### `claude.session.turn_finished`

Fires when a Claude Code session on the Claude Code host finishes a turn or
stops. The optional `project` argument limits it to one project. The payload
holds the session ID, short ID, project, status and transcript revision; read the
reply with `claude_session_get` (`includeResult`). No prompt or reply text
travels in callbacks.

While any subscription for it is active and the host link is online, the
`ClaudeSessionEvents` task lists the host's running sessions every 15 seconds and
compares each with the last state Omni recorded. A settled session whose
revision advanced, or that was working when last seen, produces one event per
session and revision. A session seen for the first time only sets the baseline.
`claude_session_start` and `claude_session_send` record the turn they begin, so
a turn that ends before the next poll still fires. A session that stops mid-turn
leaves the running list and is read individually. Turns last seen more than a
day ago do not fire when polling resumes.

## Delivery

Publishing writes the receipt and outbox rows in one transaction, then wakes the
delivery worker, so a subscriber normally hears about new mail seconds after
IMAP IDLE reports it. For email this happens before the IMAP cursor commits.
Boot recovery and a scheduled 30-second sweep retry pending work without
changing event IDs. Outbox state changes hold one short lock; webhook calls and
Executor authorization checks run outside it, so a slow callback never delays
email dispatch.

Finished deliveries are kept for seven days, receipts for 30 days (beyond the
IMAP seven-day INTERNALDATE guard, so replays stay deduplicated) and
subscriptions for seven days after they expire.

## Subscription and callback security

Subscription identity binds the authenticated owner, canonical arguments,
event name and callback URL. Refresh uses the same identity. Unsubscribe is
idempotent and scoped to that owner. Expired or revoked subscriptions do not
receive mail.

The adapter pins the configured Executor owner and validates the current OAuth
session. Omni encrypts forwarded OAuth credentials at rest and checks them
again through the fixed internal `OMNI_EVENTS_EXECUTOR_AUTH_URL` before delivery.
No user-supplied authorization URL is accepted. The storage encryption key is
derived from `OMNI_MCP_TOKEN` for this purpose. Rotating that token invalidates
old event credentials; resubscribe after rotation.

Executor 1.6.10 locks Better Auth 1.6.22, which issues one-hour MCP access
tokens and whose session check returns `null` once a token expires. Omni never
delivers without a stored token that validates at delivery time, checked once per
subscription per delivery pass. When it does not validate, all of that
subscription's pending events stay `withheld` and are rechecked after 15 minutes;
when Executor cannot answer, after one minute. Other subscriptions keep
delivering. A subscription refresh that stores a validating token releases
withheld events immediately. Withheld events fail only when their subscription
expires, is removed or is replaced by a new generation.

So that a valid token is normally present, a delegated subscription's
`refreshBefore` is the earlier of its granted lifetime and one minute before the
expiry of the token it was created or refreshed with, never earlier than the
request and never later than that expiry. Each refresh revalidates the client's
current token through the adapter. The stored subscription lifetime (24 hours by
default, at most seven days) is unchanged, so an event that arrives after the
token expires and before a late refresh stays withheld rather than failing or
being delivered.

Callback signing uses the Standard Webhooks HMAC format over the stable delivery
ID, fresh Unix signing timestamp, and exact JSON body bytes. Secrets must decode
from `whsec_` base64 to 24–64 bytes. Secret rotation briefly signs with both keys.
Callback verification uses a fresh random challenge and constant-time echo
comparison before any application data is sent.

Callbacks must use HTTPS without URL credentials or fragments. DNS is checked
at socket lookup, including all returned addresses; the connection uses those
validated addresses and retains normal hostname/TLS verification. Private,
loopback, link-local, special-purpose and non-global IPv6 ranges are blocked.
Redirects and automatic HTTP retries are disabled. Requests time out after ten
seconds, responses are bounded to 4 KiB, and event bodies to 256 KiB. Durable
outbox retry policy owns retries; HTTP 410 and 413 are terminal.

## Diagnostics

The read-only `events_status` MCP tool reports what Omni has observed for every
event: the
30 most recent `events/list`, `events/subscribe` and `events/unsubscribe`
requests with outcomes, up to 20 subscriptions with event, arguments, state, callback hostname and
advertised `refreshBefore`,
delivery counts including `withheld`, and the ten most recent deliveries with
HTTP status, transport error, failure or `withheld` reason. Owners appear as a 12-character hash prefix. It
never returns secrets, bearer tokens, callback paths or message content.
`server/discover` and `tools/list` are answered by the adapter and do not appear
here; an `events/list` row is the first Omni-side evidence of discovery. Requests
rejected by parameter validation or an invalid delegated-owner header fail before
logging, so a missing `events/subscribe` row does not prove ChatGPT never called.

## Connected verification still required

A healthy deployment and successful raw MCP calls do not prove an idle dot wakes.
The parent must use the existing Executor connection to:

1. Open the existing Executor connection in ChatGPT Plugins and select **Refresh**.
   Confirm `email.received` is listed alongside the Executor tools, then start a
   new Work conversation. This is the documented developer-mode
   [metadata refresh procedure](https://developers.openai.com/plugins/deploy/connect-chatgpt#refresh-metadata).
   Refreshing Omni's tool inventory inside Executor does not refresh ChatGPT's
   native event-source catalog. Published plugins follow continuous review.
   Confirm `events_status` shows a new `events/list` request; if it does
   not, ChatGPT did not reach the adapter with MCP 2026-07-28 discovery.
2. Create an explicit Inbox or Archive subscription in the intended Work chat
   or dot. Confirm `events/subscribe`, successful challenge verification and
   durable subscription acceptance (`accepted` or `refreshed` in
   `events_status`). Over the next hours, confirm `refreshed` rows arrive
   about hourly with an advancing `refreshBefore`; repeated refreshes without
   progress mean ChatGPT is reusing an unrotated token.
3. Select a harmless test message or send a new test message to the chosen
   folder. Confirm a matching outbox delivery receives HTTP 2xx and the intended
   chat receives the identifiers. Fetch content with `email_get` if needed.
4. Confirm a message outside the selected folder does not trigger it. Repeat a
   source observation and confirm it does not create another event ID.
5. Stop monitoring and confirm `events/unsubscribe` stops subsequent delivery.

Native automation setup belongs to the parent. Do not archive real mail to test
this integration without selecting and authorizing the exact test messages.

On 2026-10-01 the public authenticated protocol checks passed: modern discovery,
event definitions, the six native Executor tools, legacy client tools, OAuth
challenge preservation, and callback rejection before unsafe network access.
ChatGPT still listed only GitHub as a native event source; its connection refresh
was not completed. No native subscription or idle-dot delivery is proven. Keep
the parent's hourly fallback until that complete lifecycle succeeds.
