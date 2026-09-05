# omni-notify

Personal automation service for livestream monitoring, email processing, AI
briefings, recommendations, PressPods, and durable Omni workspaces.

Update this file when work establishes or changes a durable project convention.

## Work and delivery

Use `pnpm` from the repository root. After code changes, run:

```bash
pnpm check:write
pnpm test
pnpm build
```

Completed, reviewed work is authorized to ship directly to `main`: preserve
unrelated changes, commit the scoped result, pull with rebase if the remote moved,
and push. A push deploys production automatically. Do not ask for another
confirmation already supplied by this rule or the current request.

Production runs as the `omni-notify` container on `boris` (`10.10.1.100`) from
`/home/michael/compose`; persistent data is under
`/home/michael/compose/volumes/omni-notify`. The LAN UI is
`http://omni.boris/`. Inspect failures with `docker logs omni-notify` over SSH.

Prefer a committed boot migration for reproducible data changes. For one-off
database surgery, stop the container and make a WAL-safe backup first. Direct
destructive production data changes require authorization specific to that
change; reuse authorization already given in the conversation.

## Effect-first TypeScript

Effect is the application runtime for I/O, failure, concurrency, resources,
time, and randomness. Keep deterministic transformations as ordinary functions.

- Return `Effect` from production functions that perform I/O, mutate state,
  retry, sleep, acquire resources, or can fail.
- Keep `Promise` and `async` inside the smallest adapter required by an external
  framework or library. Interpret an Effect once at that edge using `src/effect/`.
- Model expected failures with domain-specific `Data.TaggedError` types. Wrap
  Promise and throwing APIs with `Effect.tryPromise` or `Effect.try` at the leaf.
- Use `Effect.fn` for reusable workflows. Add a `Context.Service` and `Layer`
  only for a real runtime or test seam.
- Decode untrusted external, persisted, environment, AI, and protocol values
  with `Schema`. Zod may remain at a protocol edge that requires Standard Schema.
- Use Effect scheduling, structured concurrency, state, cache, and scoped
  resource APIs instead of raw timers, `Promise.all`, fire-and-forget work, or
  manual cleanup chains.
- Test Effects with `@effect/vitest` and `TestClock`. Assert expected typed
  failures through `Result`; use `Exit` for interruption and defects.
- Make notifications, queue operations, external writes, and retries idempotent
  before reporting success.

For Effect 4 API details, use the installed package source and the upstream
Effect 4 migration guide. Keep ecosystem package versions aligned with `effect`.

## Architecture seams

- `src/index.ts`: entrypoint and scheduled-task registration.
- `src/effect/`: shared Effect adapters and runtime seams.
- `src/live-check/`: aggregate streamer state and viewer metrics.
- `src/email/`: transport, dispatch, activity, triage, retry, and watchdog.
- `src/parcel-tracker/` and `src/calendar-events/`: email handlers.
- `src/press-pods/`: article retrieval, narration, speech, storage, and RSS.
- `src/recommendations/` and `src/podcast-recs/`: media recommendations.
- `src/task-runs/`: durable run history and captured logs.
- `src/workspaces/`: durable conversational project workspaces.
- `src/mcp/`: authenticated, bounded adapters over personal services.
- `frontend/`: Vite and React UI.

Generic scheduling lives in `@micthiesen/mitools/scheduling`. Extend mitools
when a primitive is genuinely shared; keep project-specific behavior here.

## Failure-sensitive invariants

### Livestreams

A streamer is an aggregate identity over platform bindings. Notify only on the
aggregate offline-to-live and live-to-offline edges. The first live binding is
the sticky primary for the session; a primary switch is silent. Viewer counts
sum currently live bindings. `channels.json` is the source of truth and invalid
configuration must fail boot rather than silently unmute a streamer.

`tier: "background"` mutes live, offline, and title notifications, records only
all-time viewer highs, and polls every third tick. It cannot be combined with an
explicit `liveNotifications` value. Title changes use an eager debounce: the
first change sends immediately, later changes within ten minutes collapse to the
last title, and offline or primary-switch transitions clear pending state. A
viewer peak becomes a record only after count falls 5 percent below it; flush a
pending peak when the stream goes offline.

### Email

- Dispatch is no-drop: events received during processing schedule another pass,
  and transport cursors commit only after dispatch. Message-ID is the stable
  identity across folder moves.
- iCloud IMAP uses per-folder UID cursors without CONDSTORE/QRESYNC. Re-read
  capabilities after authentication. Keep the seven-day INTERNALDATE guard that
  cursor-skips bulk imports, and on UIDVALIDITY change replay only from the
  last-dispatch watermark before reseating the cursor.
- Filter in this order: user block, user allow, static blacklist, static
  auto-pass, then shared LLM triage. Explicit user rules override built-ins.
- Activity outcomes must reflect per-item success. A fully rejected submission
  cannot be recorded as processed.
- Parcel extraction separates order numbers from tracking numbers, validates
  ranked carrier candidates against the live list, and uses durable dedup.
- Calendar output is sanitized before persistence. Cancellations require an
  explicit event reference; receipts and bills never imply cancellation.
- CalDAV discovery follows RFC 6764 from principal to home set to a VEVENT
  collection. Never hardcode an iCloud `pXX` shard. Cross-calendar moves can
  return 403 and require delete plus recreate.
- Network and 5xx failures enter the durable retry queue. Re-fetch by email id;
  handler dedup makes replay safe.

### Notifications and runs

Throttle each notification path at exactly one layer. Preserve distinct incident
keys for distinct users, URLs, subjects, or tracking numbers. Logs emitted during
a tracked task run remain attributable through async work and are bounded before
persistence.

### PressPods

Retrievers run independently and the metadata model selects the best usable
article. Speech is chunked and verified before the finished episode is exposed.
Every sample-rate conversion in `audioChain.ts` must retain `RESAMPLE_HQ`; the
Higgs denoise path retains `FIZZ_SHELF`. See `docs/presspods-audio.md` before
changing the audio chain.

Higgs denoise uses the `arnndn` filter with
`assets/press-pods/denoise.rnnn`; the model must remain in the Docker image and
runtime FFmpeg must include that filter. Keep the pnpm override that maps
`@postlight/parser`'s git-pinned `difflib` dependency to the npm package because
the supply-chain policy blocks git dependencies.

### Workspaces and MCP

Workspace rows are changed through their service/API, not direct database edits.
Pending actions, Marketplace publishing, buyer messages, offers, address
disclosure, and meetup arrangements require user authorization. Research and
drafting do not authorize those actions. Keep MCP tools bounded adapters over
existing services and preserve strong bearer-token validation.

## Code and tests

- Oxfmt, 88 columns, two-space indentation; Oxlint for correctness checks.
- Prefer strong types, discriminated unions, small modules, and explicit returns.
- Do not leave debug logs, commented-out code, or unnecessary abstractions.
- Use Vitest and `@effect/vitest`. Test pure decisions separately from network
  adapters. Use temporary scripts for real integrations and delete them afterward.

For ad-hoc scripts that need configured credentials:

```bash
npx dotenvx run -- bun /tmp/omni-notify-<subject>.ts
```

## Secret files

`.env` contains unrecoverable secrets.

- Never print or read secret values into the conversation. List key names with
  `cut -d= -f1 .env`; test presence with an opaque `grep -qE` check.
- Never rewrite, delete, or edit existing `.env` lines. If an existing value must
  change, ask Michael to edit it.
- New placeholders may be appended. `.env.example` is version controlled and can
  be edited normally.
- Consume secrets through `dotenvx`; do not export or interpolate them into shell
  commands.

For web requests made with curl or an equivalent raw client, set the user agent
to `OpenAI File Downloader, XaiImageApiFetch/1.0`.
