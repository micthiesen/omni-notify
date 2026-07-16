# Castro sync: integration notes

Status: **investigation pending**. `src/podcast-recs/castro/client.ts` is a stub
returning `null`; the podcast-recs pipeline runs against fallbacks (below) until
a real client exists. The contract to implement is `PodcastAccountClient` in
`src/podcast-recs/account.ts`.

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

## Investigation avenues (roughly in order)

1. **MITM the app**: run the iPhone through mitmproxy (or Proxyman) with a
   trusted root cert and capture the app's traffic — endpoints, auth headers,
   payload shapes. Risk: certificate pinning; check first with a passive look
   at hostnames. This is the fastest way to learn the protocol.
2. **Extract the credentials from iCloud Keychain**: the sync credentials are
   in the keychain. iOS keychain items generally aren't visible in macOS
   Keychain Access unless the item is shared/synced with macOS accessibility;
   may require a jailbroken device or the MITM route instead. If the MITM
   capture shows a long-lived token, that alone may be enough.
3. **castro.fm web surface**: public pages exist per show/episode
   (`castro.fm/podcast/...`, `castro.fm/episode/...`). Even without auth these
   give us **deep links** for notifications, and are worth checking for JSON
   endpoints behind the pages.
4. **Sideload endpoints**: Castro Plus supports sideloading audio; if that goes
   through an authenticated upload endpoint it is another window into the auth
   scheme.

Watch out for: credentials rotating when sync re-keys, app updates changing the
event schema, and rate limiting. Whatever lands should treat transport failures
as `unavailable` (never as empty state) per the project's three-state rule.

## Fallbacks in use until this lands

| Capability | Fallback |
|---|---|
| Subscriptions (exclusion + taste input) | OPML export from Castro → `PODCAST_SUBSCRIPTIONS_PATH` |
| Listen history / outcome labeling | Skipped entirely; explicit good-pick/not-for-me feedback via the web UI |
| Enqueue episode ("acquisition") | Notification deep-links the episode page; queueing is one manual tap |
| Subscribe to show | Same — manual via deep link |

To refresh the OPML: Castro → Settings → User Data → Export Subscriptions, then
drop the file at the path `PODCAST_SUBSCRIPTIONS_PATH` points to. Staleness is
tolerable — it only weakens already-subscribed exclusions slightly.
