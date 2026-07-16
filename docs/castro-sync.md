# Castro sync: integration notes

Status: **authenticated client implemented and live-read verified**.
`src/podcast-recs/castro/client.ts` reads subscriptions, the ordered queue, and
180 days of playback history. It can enqueue a resolvable episode at the front
or back of the queue. Arbitrary-show subscription is the remaining bonus gap
because the mutation requires a Castro UUID and no feed-URL resolver has been
captured yet.

This document separates directly observed behavior from inference. Typed schemas
for captured payloads live in `src/podcast-recs/castro/protocol.ts`; HMAC signing
primitives live in `src/podcast-recs/castro/auth.ts`.

## What we know (as of July 2026)

- Castro (Bluck Apps) shipped iPad support + cross-device sync in August 2025.
  Sync covers listening progress, playback history, episode states, queue,
  folders, and podcast settings. Sideloads and appearance prefs do not sync.
- The sync engine is **event-driven CRDT**: clients exchange events through
  Castro's servers rather than polling full state. Queue and episode states are
  stored **on Castro's servers**, linked to subscriptions.
- Auth: there is **no user-visible account or password**. Each installation
  gets auto-created credentials on Castro's server, stored in the user's
  **iCloud Keychain** (this is how a second device joins the same account).
- There is **no public API** and no official third-party integration surface.

Sources:
- https://castro.fm/blog/device-sync-and-ipad (sync architecture + keychain credentials)
- https://support.supertop.co/ (support site; OPML export docs)

## Implications

- A server API **exists** (the app speaks HTTPS to Castro's backend); it is just
  private and device-credential-authenticated. Read/write is plausible if we
  can obtain one installation's credentials and mimic its requests.
- Because sync is CRDT-event-shaped, "fetch listen history" may not be a single
  REST GET. The client implementation may need to ingest the event stream and
  maintain a local replica, then answer the snapshot-shaped reads of
  `PodcastAccountClient` from that replica. Callers only ever see snapshots.

## Captured private API protocol

Captured July 16, 2026 from Castro iOS build 2396 with Proxyman, using the
owner's own Castro Plus account. Raw HAR files are intentionally not committed:
they contain a live access ID, request signatures, subscription identifiers,
and personal podcast activity.

### Host and common headers

All observed private API requests use `https://tentacles.castro.fm` and include:

| Header | Observed value or role |
|---|---|
| `Accept` | `application/vnd.tentacles.supertop.co+json; version=8` |
| `Content-Type` | `application/json`, including bodyless GETs |
| `X-Tentacles-App` | `castro-ios` |
| `X-Tentacles-Platform` | `iOS` |
| `User-Agent` | `Castro/2396 CFNetwork/3890.100.1 Darwin/27.0.0` |
| `Date` | RFC 7231 HTTP date, used by request signing |
| `X-Authorization-Content-SHA256` | Base64 SHA-256 of the exact request body; bodyless requests hash the empty string to `47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=` |
| `Authorization` | `APIAuth-HMAC-SHA256 <access-id>:<base64-signature>` |

The access ID is a UUID. The 44-character signature is a Base64-encoded
HMAC-SHA256 digest. The scheme matches the `api-auth` canonical form:

```text
METHOD,CONTENT_TYPE,CONTENT_SHA256,PATH_AND_QUERY,DATE
```

The canonical form was validated byte-for-byte against a captured request. The
full path **including the query string** is signed. The keychain secret is used
as its stored UTF-8 text, not Base64-decoded first. A fresh signed
`GET /profile/sync/status` made outside Proxyman returned 200, proving the
credential is sufficient on its own.

### Read and sync endpoints

| Method and path | Observed response |
|---|---|
| `GET /ping` | 200, empty body |
| `GET /profile/sync/status` | `{ device_status: number, account_status: number, latest_event_id: number }` |
| `GET /profile/events?since=<id>&limit=<n>` | `{ events: unknown[], latest_event_id: number }`; captured at the current cursor with an empty array |
| `GET /profile/sync/user_events?since=<id>&limit=<n>` | `{ user_events: unknown[], latest_event_id: number }`; captured at the current cursor with an empty array |
| `GET /profile/sync/podcast_state?…` | Podcast UUID plus per-episode new/starred/played/progress/last-played state |
| `GET /profile/subscriptions` | Current subscriptions as `{ podcast_id, private, will_notify_device }[]` |
| `GET /profile/sync/queue` | `{ queue_items: [{ fractional_position, episode_id, podcast_id }] }` |
| `GET /profile/sync/podcast_settings/<podcast-id>` | 404 for a show without custom settings |
| `GET /podcasts/<podcast-id>` | Public podcast metadata plus its episode records |
| `GET /episodes/<episode-id>` | Full metadata for one episode, including RSS GUID and duration |
| `GET /podcast_notes/<podcast-id>` | `{ notes: string }` |
| `GET /transcripts/<episode-id>` | 404 for both captured episodes |
| `GET /download/<short-id>` | 302 to episode media |

`GET /podcasts/<podcast-id>` uses the same UUID that subscription mutations
call `feed_id`. Its podcast object contains:

```text
public_id, short_id, title, sort_title, site_url, description, author_name,
artwork_url, last_event_number, podcast_type, itunes_category,
itunes_subcategory, private, funding_text, funding_url, episodes, people
```

Captured episode objects contain:

```text
guid, public_id, short_id, title, media_size, media_url, artwork_url,
author_name, link_url, duration.seconds, description, published_at,
predecessor_public_id, season_number, episode_number, episode_type, people
```

This provides the mapping needed by the public contract: Castro `public_id` is
the native episode ID, while `guid` maps to the RSS item GUID.

Live probing after authentication found populated `podcast_state` responses.
Each `episode_states` item has:

```text
episode_id, is_new, is_starred, is_played, last_played, progress_seconds
```

`last_played` is a nullable ISO timestamp. The client fetches current
subscriptions, loads each podcast's state, keeps activity from the last 180
days, and resolves the retained episode IDs through `/episodes/<id>`. It then
computes completion from `progress_seconds / duration.seconds`, using 1 for an
explicitly played episode. Metadata promises are cached for the lifetime of the
service and reads are capped at eight concurrent requests. This live history is
currently scoped to podcasts that remain subscribed. The backup proves older,
unsubscribed history exists locally, but the live API has not exposed a route
that lists every historical podcast ID.

### Action write endpoint

User and policy changes are batched to:

```http
POST /profile/sync/actions
Content-Type: application/json

{ "actions": [ ... ] }
```

Each observed action contains:

```text
id                 monotonically increasing device-local integer
episode_id         Castro episode UUID
origin_event_id    newly generated UUID per action
origin_timestamp   epoch milliseconds
source             "user" or "policy"
action_type        event name
event_data         optional action-specific JSON encoded as a string
```

The endpoint returned 200 with an empty body for every captured batch.
The reversible live write also confirmed that a process-local monotonic action
ID seeded from epoch milliseconds is accepted; `origin_event_id` remains the
globally unique idempotency identity.

Captured user flows:

| UI operation | Actions, in order |
|---|---|
| Queue Next | `episode_queued` with `event_data={"fractional_position":"ZME"}`, then `clear_episode_new` |
| Queue Last | `episode_queued` with `event_data={"fractional_position":"aE"}`, then `clear_episode_new` |
| Queue and begin playback | `episode_queued` with position `ZM`, then `episode_last_played` with epoch-seconds `last_played` |
| Pause after about four seconds | `episode_progress` with floating-point `seconds` (`3.8196825396825398` captured) |
| Clear from queue | `episode_dequeued`, then `clear_episode_new` |

The captured position strings are examples, not constants. They are
fractional-ordering keys computed relative to the existing queue. A client must
first reconstruct the ordered queue, then generate a key before the first item
for `next` or after the final item for `last`. `EnqueueEpisodeRequest.position`
exposes these two choices through `PodcastQueuePosition`. The implementation
uses the `fractional-indexing` package. A live reversible smoke test added an
already-played episode at Queue Next, confirmed it became the first item, and
dequeued it again, restoring the original 29-item queue.

### Subscription mutations

Subscribe and unsubscribe both accept a Castro podcast/feed UUID, not an RSS
URL directly:

```http
POST /profile/subscriptions/subscribe
POST /profile/subscriptions/unsubscribe
Content-Type: application/json

{ "feed_ids": ["<podcast-public-id>"] }
```

Subscribe returned 201 with:

```json
{
  "subscribed": [{ "feed_id": "<uuid>", "feed_url": "https://…" }],
  "latest_event_id": 123
}
```

Unsubscribe returned 200 with an empty body. Subscribing also produced an
`episode_new` action with `source: "policy"` for the inbox episode selected by
the show's policy. Unsubscribing produced `clear_episode_new` with
`source: "user"` for that episode.

### Other captured request

On launch, Castro called `POST /unlock/subscription` with App Store
`transaction_id` and `original_transaction_id` query parameters. This appears
to refresh Castro Plus entitlement and is not required for the account-client
contract. Never log or commit those identifiers.

## Unknowns required for a complete client

1. Identify the endpoint that resolves an RSS feed URL to Castro's
   podcast/feed UUID. Subscription and enqueue requests cannot use the public
   contract's RSS identity without this mapping. Enqueue currently uses a
   normalized show-title match among subscriptions, then an RSS GUID match in
   that podcast's metadata.
2. Capture populated incremental `events` and `user_events` payloads. They are
   no longer required for account snapshots because dedicated live read routes
   were discovered, but documenting them would complete the protocol picture.

## Credential extraction and durability

The credential was recovered legitimately from the owner's encrypted Finder
backup after the iPhone was connected and trusted. The following files were
inspected in a mode-600 temporary directory and are not part of the repository:

```text
Castro.sqlite
sync_ledger.db
Castro-preferences.plist
keychain-backup.plist
```

The matching keychain records are in access group
`6N27F5GP4R.co.supertop.castro-keychain` with accounts:

```text
castro.account.device.deviceId
castro.account.device.secretKey
```

The access ID is a UUID and the secret is an 88-character string. Neither value
is documented or committed. They are stored as `CASTRO_ACCESS_ID` and
`CASTRO_SECRET_KEY` in the ignored local `.env`, whose permissions were changed
to 0600. Routine requests do not need Proxyman, the iPhone, iCloud access, or a
new Finder backup. Re-extraction is only expected if Castro revokes or rotates
the device credential, the account is reset, or the keychain item is deleted.

### Backup database inventory

The one-time backup provided a useful cross-check of the server snapshot:

| Item | Captured count |
|---|---:|
| `SUPPodcast` rows | 178 |
| Currently subscribed (`subscribedState = 0`) | 40 |
| `SUPEpisode` rows | 55,611 |
| `SUPEpisodeQueuedState` rows | 29 |
| `SUPEpisodePlayedState` rows | 1,138 |
| Episodes with `lastPlayed > 0` | 1,655 |
| Episodes with `progress > 0` | 670 |
| Starred episodes | 2 |
| `SUPPlaySession` rows | 4,878 |

The live API subsequently returned 39 subscriptions and the same 29 queue
items. The one-show difference is consistent with the subscribe/unsubscribe
capture occurring after the backup. Relevant local mappings are:

- `SUPPodcast.publicId` is Castro's podcast UUID.
- `SUPEpisode.publicId` is Castro's episode UUID.
- `SUPEpisodeQueuedState.fractionalPosition` is the queue ordering key.
- `SUPEpisode.progress`, `lastPlayed`, and `starred` hold current playback state.
- `SUPPlaySession` records session-level played-from/to ranges and timestamps.
- `sync_ledger.db.sync_action_item` is the pending outbound action ledger.

Watch for credentials rotating when sync re-keys, app updates changing the
schema, and rate limiting. Transport failures are returned as `unavailable`,
never as empty state, per the project's three-state rule.

## Remaining fallbacks

| Capability | Fallback |
|---|---|
| Subscriptions | OPML remains the fallback when the live account read is unavailable |
| Listen history | Explicit feedback remains available if the account read is unavailable |
| Enqueue episode | Deep link remains the fallback when the show/GUID cannot be resolved |
| Subscribe to show | Manual deep link until RSS URL to Castro UUID resolution is implemented |

To refresh the OPML: Castro → Settings → User Data → Export Subscriptions, then
drop the file at the path `PODCAST_SUBSCRIPTIONS_PATH` points to. Staleness is
tolerable — it only weakens already-subscribed exclusions slightly.
