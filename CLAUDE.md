# CLAUDE.md

> This is a living document. Update it when you learn new preferences, patterns, or project conventions. Don't ask—just update it if something is missing or outdated.

## Project Overview

**omni-notify** monitors livestream platforms (YouTube, Twitch), emails (via Fastmail JMAP), and more — sending Pushover notifications and taking automated actions. Features include live-check notifications, AI briefing agents, parcel tracking from emails, and automatic calendar event creation from emails.

## Quick Reference

```bash
pnpm dev      # Development with hot reload
pnpm build    # TypeScript compilation (run after changes)
pnpm test     # Run tests (vitest)
pnpm check    # Biome linting + formatting check
```

**Always run `pnpm check:write && pnpm test && pnpm build` after making changes.**

## Architecture

```
src/
├── index.ts                 # Entry point, registers scheduled tasks
├── scheduling/              # Generic scheduling infrastructure
│   ├── Scheduler.ts         # Cron management, graceful shutdown
│   └── ScheduledTask.ts     # Abstract base class for tasks
├── live-check/              # Livestream monitoring feature
│   ├── task.ts              # LiveCheckTask: aggregate per-streamer loop
│   ├── transitions.ts       # Pure state machine: decides live/offline/title edges
│   ├── sessions.ts          # Completed live-session history (start/end/peak/title)
│   ├── streamers.ts         # Streamer model: merges bindings by display name
│   ├── channelsConfig.ts    # Loads per-streamer overrides from channels.json
│   ├── persistence.ts       # Streamer live/offline state (SQLite)
│   ├── platforms/           # Platform implementations
│   │   ├── index.ts         # Platform enum, types, config registry
│   │   ├── common.ts        # Shared fetch utilities
│   │   ├── youtube.ts       # YouTube HTML scraping
│   │   ├── twitch.ts        # Twitch GQL API
│   │   └── kick.ts          # Kick official public API (OAuth client credentials)
│   └── metrics/             # Viewer metrics with rolling windows (per streamer)
│       ├── ViewerMetricsService.ts  # Peak confirmation state machine
│       ├── persistence.ts   # ViewerMetricsEntity (daily buckets, keyed on streamerId)
│       └── windows.ts       # Rolling window calculation helpers
├── ai/                      # AI model configuration and shared tools
│   ├── registry.ts          # Provider registry (Google, Anthropic, OpenAI)
│   └── tools/               # Shared AI agent tools (reusable across any agent)
│       ├── webSearch.ts     # Tavily web search tool
│       └── fetchUrl.ts      # URL fetcher: HTML → clean markdown via Readability + Turndown
├── briefing-agent/          # AI-powered briefing tasks (web search → notify)
│   ├── BriefingAgentTask.ts # Config-driven task class
│   └── configs.ts           # Loads briefing configs from BRIEFINGS_PATH .md files
├── jmap/                    # Shared Fastmail JMAP infrastructure
│   ├── client.ts            # JMAP session + account resolution
│   ├── activity.ts          # Per-email pipeline outcome records (filtered/processed/error)
│   ├── dispatcher.ts        # EmailDispatcher: fetch-once fan-out to EmailHandlers
│   ├── eventSource.ts       # SSE for real-time email state changes
│   ├── emailFetcher.ts      # Fetch new emails via JMAP changes API
│   ├── persistence.ts       # Shared JMAP email state cursor (SQLite)
│   └── htmlToText.ts        # HTML email body → plain text
├── parcel-tracker/          # Auto-submit tracking numbers from emails to Parcel.app
│   ├── index.ts             # Pipeline factory
│   ├── pipeline.ts          # Email → filter → LLM extract → Parcel API
│   ├── persistence.ts       # Dedup gate + JMAP state (SQLite)
│   ├── extraction/          # LLM tracking number extraction
│   ├── filter/              # Email candidate filtering (keywords, carriers)
│   ├── carriers/            # Parcel API carrier list + blacklist
│   └── parcel/              # Parcel.app API client
├── calendar-events/         # Auto-create calendar events from emails via CalDAV
│   ├── index.ts             # Pipeline factory
│   ├── pipeline.ts          # Email → filter → LLM extract → CalDAV PUT
│   ├── persistence.ts       # Dedup gate + JMAP state (SQLite)
│   ├── extraction/          # LLM calendar event extraction
│   ├── filter/              # Email candidate filtering (booking/travel keywords)
│   └── fastmail/            # CalDAV calendar API (raw iCalendar over HTTP)
├── recommendations/         # AI media recommendations → watchlist + Pushover
│   ├── task.ts              # RecommendationTask (cron, default Mon/Wed/Fri 5pm)
│   ├── pipeline.ts          # Poll state → outcomes → candidates → filter → shortlist → select → commit
│   ├── mediaLibrary.ts      # Plex history/in-progress/library bridge
│   ├── watchlist.ts         # Combined Radarr/Sonarr tracked-state + acquisition bridge
│   ├── tmdb/                # TMDB client (canonical identity + candidate sources)
│   ├── identity.ts          # GUID → tmdb:{type}:{id} resolution, cached in SQLite
│   ├── outcomes.ts          # Pure outcome labeling (watched ≥80% / abandoned / ignored 30d)
│   ├── candidates.ts        # Pool assembly with source quotas + novelty bucket
│   ├── filters.ts           # Pure hard filters (pre-model)
│   ├── shortlist.ts         # Cheap-model scoring, composite computed in code
│   └── selection.ts         # Strong-model research (Tavily tools) + structured decision
├── podcast-recs/            # AI podcast-episode recommendations → Pushover
│   ├── task.ts              # PodcastRecommendationTask (cron, default Mon/Wed/Fri 11am;
│   │                        #   PODCAST_TASTE_PATH doubles as the feature flag)
│   ├── pipeline.ts          # Two-tier: guest appearances (Tier 1) + topic/drama (Tier 2)
│   │                        #   → verify → filter → gate/select → commit (pending→enqueue→notify)
│   ├── account.ts           # PodcastAccountClient bridge contract
│   ├── castro/              # Castro private-sync client + hourly Inbox cleanup
│   ├── voices.ts            # Parse ## Voices (followed people) from the taste md
│   ├── guests.ts            # Tier-1 discovery: PI byperson + Tavily person-search fallback
│   ├── guestSelection.ts    # Tier-1 default-include gate (capped, for press-tour weeks)
│   ├── podcastindex/        # Podcast Index API client (byperson guest search)
│   ├── discovery.ts         # Tier-2 Tavily multi-angle (topic/drama) → candidate extraction
│   ├── candidates.ts        # iTunes (Castro fallback) + RSS resolution; PI→candidate mapping
│   ├── itunes.ts            # Keyless iTunes Search API client
│   ├── rss.ts               # RSS episode parsing (linkedom)
│   ├── subscriptions.ts     # Castro-account subscription resolution (three-state)
│   ├── filters.ts           # Pure hard filters (7d recency, subscribed/cooldown/excluded)
│   ├── shortlist.ts         # Tier-2 cheap-model scoring, composite computed in code
│   ├── selection.ts         # Tier-2 strong-model research + structured one-pick decision
│   ├── outcomes.ts          # Pure listen-history outcome labeling
│   ├── taste.ts             # Seed profile + subscriptions + feedback + reflection digest
│   ├── reflection/          # Versioned podcast taste reflection (weekly task):
│   │                        #   Castro listen history + rec outcomes → evidence ledger
│   │                        #   → draft/critic LLM passes → validated profile checkpoints
│   └── persistence.ts       # PodcastRecommendationEntity, exclusions, feedback
├── task-runs/               # Generic task-run tracking (powers the web UI)
│   ├── persistence.ts       # TaskRunEntity + TaskRunLogEntity (last 50 runs/task), interrupted-run repair
│   ├── events.ts            # taskRunBus (run start/finish) + runLogBus (per-line log events)
│   ├── logCapture.ts        # Logger.onLog tap + AsyncLocalStorage run attribution
│   └── registry.ts          # TaskRegistry: tracked wrapper, manual runs, next-run times
├── emails/                  # Email utilities (general purpose)
├── tools/
│   └── preview-server.ts    # Dev harness: real server + fake data for frontend work
├── server.ts                # Hono API (/api/tasks, /api/task-runs, /api/recommendations,
│                            #   /api/pets, /api/streamers, /api/snapshot,
│                            #   /api/trigger-channels for homebridge-stream-triggers) + SSE (/api/events,
│                            #   /api/task-runs/:runId/logs/stream) + SPA
└── utils/
    └── config.ts            # Environment config with zod validation
```

Frontend (`frontend/`): React SPA ("Omni Notify") with client-side path routing — `/` dashboard (stat strip, live-streamer cards, task cards with countdowns + run history, activity feed), `/pets` weight tracker (lazy-loaded recharts chunk), `/recommendations` recommendation list with status filters, `/podcasts` podcast picks + taste brain, `/briefings` briefing-notification archive, `/emails` per-email pipeline activity (lazy), `/feedback/{recommendations|podcasts}/:id` mobile one-tap rating page (Pushover notifications deep-link here), `/streamers/:id` streamer detail (live status, 7/30/90-day + all-time viewer highs, peak-viewers-by-day bar chart from `ViewerMetricsEntity` daily buckets, recent-streams session list from `StreamSessionsEntity`; streamer cards/pills link here). StreamerPage shares the lazy recharts chunk with PetsPage. All dashboard state flows through one SSE connection (`LiveDataProvider` in `frontend/src/live.tsx`): the server serializes a full snapshot (tasks + streamers + recent runs) once per task-run start/finish and broadcasts it to all connected clients (byte-identical pushes skipped, `X-Accel-Buffering: no` so proxies don't buffer); the client fetches `/api/snapshot` immediately on mount in parallel with opening the stream and polls until the first SSE snapshot lands (first paint never waits on the stream), then falls back to polling whenever the stream is down, showing the connection state in the nav bar. Hashed `/assets/*` are served with immutable cache headers; HTML revalidates. To preview the UI with fake data: `DB_NAME=/tmp/omni-preview.db FRONTEND_PORT=3999 npx tsx src/tools/preview-server.ts`.

### Frontend Style Guide

All styling lives in `frontend/src/index.css` (plain CSS, design tokens as CSS variables in `:root`). Conventions:

- **Label casing**: every UI label starts with a capital ("Run now", "Not for me", "Notified") — never lowercase chips/labels. Dynamic identifiers (task names, data entity labels, briefing names) render through `toTitleCase()` from `utils/format.ts` ("CastroInboxCleanup" → "Castro Inbox Cleanup"); keep raw values for keys/API calls. Tiny badges may be CSS-uppercased (`.trigger-badge`, `.section-title`).
- **Tokens, not literals**: colors via `--bg-*`/`--text-*`/`--accent`/`--success`/`--danger`/`--warn`/`--live`; radii via `--radius` (cards/modals), `--radius-sm` (buttons, inputs, tiles, tags that wrap long text), `--radius-xs` (tiny badges, code chips); `999px` only for true pills with short single-line text. Header controls (buttons, selects) share `--control-height`.
- **Metadata rows** ("Jul 16, 8:20 PM · 12s · Manual"): use `.meta-row` with each part as its own child element. Separator dots are generated by CSS (`::after`), never literal `·`/`&middot;` in JSX — this keeps wrapping clean on mobile (no line ever starts with a dot).
- **Date/time formatting**: only via `utils/format.ts` (`formatAbsolute`, `formatAbsoluteWithYear`, `formatDateOnly`, `formatRelative`, …). No per-page `toLocaleString` helpers.
- **Task output is log-like** → `.run-summary`/`.run-error` render in `--font-mono` wherever they appear (task cards, history, activity, run lists).
- **Cards in grids** stretch to equal row height (no `align-items: start`); interactive rows inside padded cards bleed hover highlights to the card edge (negative `margin-inline` + matching `padding-inline`).
- **Pages are full width**: no per-list `max-width`; page headers use `.page-header` with `.page-header-stack` (title + `.page-subtitle` stacked left), actions on the right.
- **Repeated-column lists** (e.g. `.rec-run-list`) align columns across rows via parent grid + `grid-template-columns: subgrid` on rows; free-form feeds (dashboard Activity) stay flex.
- **Filter chips**: always `.chip-btn` (+`.chip-btn-count` for counts); status indicators always reserve their space (`.status-dot.status-none` when idle) so labels don't shift.
- **Shared components** live in `frontend/src/components/`: `TasteBrain` (both rec pages, always rendered at the top of the page), `StatusFilterChips`, `ImageWithFallback`, `badges.tsx`. Extend these rather than copy-pasting between the Recommendations and Podcasts pages.

### Recommendations Design Invariants

- Taste inputs use ground-truth Plex watch history plus explicit good-pick/not-for-me feedback. Passive outcome labels remain bookkeeping only.
- "Watched" requires ≥80% completion (`WATCHED_COMPLETION_THRESHOLD`); partial starts that stall become "abandoned" (a negative signal for exclusions, not prompts).
- Each recommendation attempt has a unique `recommendationId`; `canonicalId` remains content identity for cooldowns and exclusions.
- The RecommendationEntity row is written as `pending` before acquisition, then flipped to `notified` after Pushover. Stale pending rows are reconciled only when acquisition demonstrably landed or the title was already available in Plex.
- Plex availability, Radarr/Sonarr tracked state, and explicit user feedback are separate concepts. Unavailable service reads abort the run rather than being treated as empty state.
- Cooldown: 180 days for re-recommendation; watched, abandoned, not-for-me, and already-watched titles are excluded permanently unless newer explicit feedback corrects the choice.

### Podcast Recommendations Design Invariants

Deliberately a sibling system to media recommendations (same architecture, separate implementation — the domains differ too much for shared types): episode-level, freshness-critical, and centered on shows the user does NOT already follow.

- **People-first, two-tier** (see `docs/podcast-recs.md`). Tier 1 is the point: episodes where a followed **voice** (from the taste md's `## Voices` section, parsed by `voices.ts`) guests somewhere new. Discovery (`guests.ts`) is Podcast Index `byperson` first (free, structured, pre-resolved) then a Tavily person-search fallback for voices it missed (covers non-podcasters). Voices rotate `PODCAST_VOICE_ROTATION_MAX`/run via a persisted cursor (`nextVoiceBatch`). Tier 1 uses a default-include gate (`guestSelection.ts`) capped at `PODCAST_MAX_GUEST_PICKS` (bursts like book tours). Tier 2 is the original topic/drama shortlist→select, conservative and suppressed once Tier 1 delivers ≥3.
- Drama/debate is a positive (Blocked-and-Reported anchor), handled prompt-side, not by filters. The line is bad-faith grift/rage-farming = out, gossip/beef/debate from sharp people = in.
- Identity: shows are `itunes:{id}` (fallback `feed:{normalized url}`), episodes are `{showId}#{rss guid}` (`types.ts`).
- Release dates are verified from the show's actual RSS feed in code — never trusted from search snippets or model output. Unverifiable candidates are dropped.
- Hard recency window: episodes older than 7 days are ineligible (`filters.ts`).
- Episodes are excluded permanently once delivered; shows get a 30-day cooldown; not-for-me feedback excludes the show permanently unless newer feedback corrects it.
- Subscribed shows are excluded (hard-filtered from the Castro account when configured, prompt-excluded via the seed profile otherwise) and double as the main taste evidence. **The exclusion is load-bearing**: a failed Castro subscription read aborts the run rather than risk recommending a followed show (three-state rule in `subscriptions.ts`).
- Taste has a fourth input beyond seed profile/subscriptions/feedback: `PodcastTasteReflectionTask` (weekly, Sunday 5am ±5min jitter) distills the full 180-day Castro listen history plus recommendation outcomes into a versioned profile (`reflection/`, mirroring `recommendations/taste/`): append-only evidence ledger, fingerprint no-op guard, draft + skeptical-critic LLM passes, and code-level claim validation (stable/conditional/saturation claims need ≥2 independent shows; one explicit not-for-me can support an aversion). The latest profile digest is appended to every pipeline prompt via `buildTasteDigest` and surfaced at `GET /api/podcast-recommendations/taste-profile` + the Podcasts page "Taste brain".
- Castro is behind the `PodcastAccountClient` bridge (`account.ts`), implemented against its captured private sync protocol (`castro/`, credentials `CASTRO_ACCESS_ID`/`CASTRO_SECRET_KEY`); see `docs/castro-sync.md`. It provides subscriptions, 180 days of listen history, general podcast/episode search, direct RSS resolution, subscription writes, and queue writes. Selected recommendations are resolved by exact RSS URL or iTunes ID and auto-enqueued at Queue Next (inserted after the current top item, matching the app) before notification; resolution or enqueue failure keeps the deep-link fallback. Episode matching against Castro uses the enclosure/media URL, not the RSS guid, which hosting platforms rewrite (see `docs/castro-sync.md`). When unconfigured the pipeline degrades to the seed profile + explicit feedback.
- `CastroInboxCleanup` runs hourly with up to five minutes of jitter. It scans `is_new` episode state and posts only `clear_episode_new` for descriptions beginning exactly with `This is a free preview`; it never dequeues the episode.
- Commit protocol mirrors media recs: write `pending` before Castro enqueue and Pushover, then flip to `notified`; stale pending rows become failed with a 24h retry exclusion. Enqueue is idempotent, so a retry observes `already_exists` if that effect landed before a crash, while notification delivery remains unverifiable.
- Well-behaved-client controls (Castro is a private, reverse-engineered API): every request funnels through ONE process-wide rate-limit queue in `CastroApi` (concurrency 4, ≤8 req/s) — `createCastroClient` shares a singleton `CastroApi` so overlapping task runs can't double the ceiling; each request is signed at send time. Scheduled tasks jitter ±5min off their cron instant. Listen-history reads for outcome sync (the heaviest fan-out) are skipped entirely when no recommendation is open, and otherwise bounded to just before the oldest open delivery — never the full 180-day window, but always wide enough to cover every open rec (a shorter cutoff would mislabel a listened episode as ignored). The weekly taste-reflection read is the deliberate exception: one full-window read per week, through the same rate-limited singleton.
- The old `PodcastPicks` briefing (`briefings/PodcastPicks.md` on the deploy host) is superseded by this feature and should be disabled when `PODCAST_TASTE_PATH` is configured.

## Key Patterns

### Three-State Status System

Providers return one of three statuses to prevent false notifications:

```typescript
enum LiveStatus {
  Live = "live",       // Confirmed live → can trigger "went live" notification
  Offline = "offline", // Confirmed offline → can trigger "went offline" notification
  Unknown = "unknown", // Network error, bad response, etc. → NO state change
}
```

**Why**: Network errors or API changes shouldn't trigger false "offline" notifications mid-stream.

### Adding a New Platform

1. Create `src/live-check/platforms/{platform}.ts` with:
   - `fetch{Platform}LiveStatus({ username })` → `Promise<FetchedStatus>`
   - `get{Platform}LiveUrl(username)` → `string`
   - `extractLiveStatus(data)` → `FetchedStatus` (for testing)

2. Update `src/live-check/platforms/index.ts`:
   - Add to `Platform` enum
   - Add to `platformConfigs` record

3. Update `src/utils/config.ts`:
   - Add `{PLATFORM}_CHANNEL_NAMES: commaSeparatedString`

4. Update `src/index.ts`:
   - Add to the `sources` array passed to `buildStreamers`
   - Add to `PLATFORM_PRIORITY` in `src/live-check/streamers.ts` (decides tiebreak order when multiple platforms go live in the same tick)

5. Update `.env.example` and `README.md`

6. Create `src/live-check/platforms/{platform}.spec.ts`

### Adding a New Scheduled Task

1. Create a new feature folder `src/{feature-name}/`

2. Create `src/{feature-name}/task.ts`:
   ```typescript
   import { ScheduledTask } from "../scheduling/ScheduledTask.js";

   export default class MyTask extends ScheduledTask {
     public readonly name = "MyTask";
     public readonly schedule = "0 17 * * *";  // 5pm daily
     // Optional: public override readonly jitterMs = 5000;

     public async run(): Promise<void> {
       // Task logic
     }
   }
   ```

3. Register in `src/index.ts`:
   ```typescript
   import MyTask from "./{feature-name}/task.js";
   scheduler.register(new MyTask());
   ```

The `Scheduler` handles cron management, prevents overlapping runs (per-task queue), and graceful shutdown.

### Adding a New Briefing Task

Briefing tasks use AI to search the web and send notification summaries. Configs are loaded from `.md` files in the folder specified by `BRIEFINGS_PATH`. To add a new topic, create a markdown file:

```markdown
---
schedule: "0 0 9 * * *"
---
You are a tech news assistant...
```

- **Filename** becomes the config name (e.g. `TechNews.md` → name `"TechNews"`)
- **Frontmatter** must contain a valid `schedule` (node-cron expression)
- **Body** is the prompt sent to the AI agent

The loop in `index.ts` auto-registers all valid configs. Invalid files are skipped with a warning. For custom behavior, subclass `BriefingAgentTask` and override `run()`.

**Prompt Placeholders:** Use placeholders in the prompt body to inject dynamic content at runtime:

- `{{date}}` → current date, e.g. `Thursday, February 6, 2026` (local timezone)
- `{{time}}` → current time, e.g. `9:00 AM EST` (local timezone)
- `{{history:N}}` → last N notifications sent by this briefing (avoids duplicate coverage)

Placeholder resolution lives in `src/briefing-agent/placeholders.ts` which chains all placeholder types via `resolveAllPlaceholders()`.

```markdown
---
schedule: "0 0 9 * * *"
---
You are a tech news assistant. Today is {{date}}, {{time}}.

{{history:10}}

Do not cover topics that appear in past notifications above.
```

History is stored per-briefing in SQLite and auto-pruned to the last 50 entries.

### Error Handling

- Providers catch their own errors and return `LiveStatus.Unknown`
- `LiveCheckTask` tracks consecutive unknowns per channel
- Escalating log levels: debug (1-2) → warn (3-9) → error (10+)
- Logger `warn`/`error` go to Pushover via @micthiesen/mitools

### Task Run Logs

Every log line emitted during a task run is captured and viewable in the web UI (click a task card's last-run line, a history row, or an activity row). How it works:

- `installLogCapture()` (called at boot) sets the global `Logger.onLog` tap from mitools ≥2.4.0. The tap fires for **every** log call regardless of `LOG_LEVEL`, so DEBUG lines reach the UI while console/compose output still respects the threshold.
- `TaskRegistry.execute` wraps `task.run()` in an `AsyncLocalStorage` context carrying the `runId`; the tap attributes lines to the active run (across awaits, sub-loggers, and concurrent tasks). Lines logged outside any run (server, JMAP pipelines) are ignored.
- In-flight lines live in a per-run memory buffer (capped at 2000 lines / 4KB per line, oldest dropped) and are broadcast on `runLogBus`. On run end the buffer is persisted as one `TaskRunLogEntity` row, pruned alongside `TaskRunEntity`'s 50-runs-per-task retention.
- API: `GET /api/task-runs/:runId/logs` (buffer if running, else stored row); `GET /api/task-runs/:runId/logs/stream` (SSE live tail: `init` replays the buffer, `line` frames follow, `done` carries the settled run). Logs never ride the dashboard snapshot stream.
- Frontend: `LogViewer.tsx` modal — level filter chips (debug hidden by default), stick-to-bottom tailing, LIVE badge while streaming.

### Persistence

Uses `@micthiesen/mitools` Entity system with SQLite (`docstore.db`):
- `StreamerStatusEntity`: Aggregate live/offline state per streamer (one row per merged identity, keyed on `streamerId`). Holds the sticky primary binding, summed max viewer count, and the per-binding titles for the current live session.
- `ViewerMetricsEntity`: Daily viewer buckets + all-time max, keyed on `streamerId`. Recorded viewer count is the **sum across currently-live bindings**.

### Streamer Model

A `Streamer` is the identity unit: display name (normalized, case-insensitive) collapses multiple `(platform, username)` bindings into one. Notifications fire on the aggregate edges:
- **went-live**: offline everywhere → live somewhere (one notification)
- **went-offline**: live somewhere → offline everywhere (one notification)
- **title change**: only when the primary binding is unchanged AND its title changed
- **primary switch** (e.g., original primary drops but another is still live): silent

Primary election is **first-to-go-live wins**, sticky for the session. Priority tiebreak when multiple go live simultaneously: YouTube → Twitch → Kick (see `PLATFORM_PRIORITY` in `src/live-check/streamers.ts`).

The pure transition logic lives in `src/live-check/transitions.ts` (`decideTransition`) for easy testing.

### Viewer Metrics System

Tracks viewer records across rolling time windows (7d, 30d, 90d, all-time) using **peak confirmation**:
- Records are only confirmed when viewer count drops 5% below peak (prevents spam during climbs)
- Pending peaks are flushed when stream goes offline
- Only sends one notification for the highest-priority window when multiple records are broken
- Recorded value is the **sum of viewer counts across a streamer's currently-live bindings**, so multistream metrics reflect total reach

## Code Style

- **Biome** for formatting (88 char line width, 2-space indent) and linting
- **2 spaces** for indentation (not tabs)
- **Strong types**: Use enums, discriminated unions, explicit return types
- **No over-engineering**: Simple solutions, no unnecessary abstractions
- **Clean code**: No debug leftovers, no `console.log`, no commented code

## Testing

Uses **vitest** for testing. Run with pnpm:

```bash
pnpm test              # Run all tests
pnpm test -- --watch   # Watch mode
```

- YouTube tests are skipped (marked `.skip`) - they need real HTML fixtures
- Twitch tests run against `extractLiveStatus` with mock data
- Test the pure extraction functions, not the network fetchers

### Ad-hoc integration scripts

For testing things that hit real APIs or URLs (e.g. verifying Tavily search, testing HTML-to-markdown on real pages), write a temporary `.ts` script and run it with `dotenvx` + `bun` so it picks up `.env` credentials:

```bash
npx dotenvx run -- bun src/tools/my-test-script.ts
```

Delete the script when done—these are throwaway, not committed.

## Environment Variables

```bash
LOG_LEVEL=info|debug|warn|error
PUSHOVER_USER=xxx
PUSHOVER_TOKEN=xxx                      # Fallback for all notification types
PUSHOVER_LIVE_TOKEN=xxx                 # Optional: override for live-check notifications
PUSHOVER_BRIEFING_TOKEN=xxx             # Optional: override for briefing notifications
PUSHOVER_PARCEL_TOKEN=xxx               # Optional: override for parcel notifications
PUSHOVER_CALENDAR_TOKEN=xxx             # Optional: override for calendar notifications
YT_CHANNEL_NAMES=@channel1,@channel2    # YouTube handles
TWITCH_CHANNEL_NAMES=user1,user2        # Twitch usernames
KICK_CHANNEL_NAMES=slug1,slug2          # Kick channel slugs
KICK_CLIENT_ID=xxx                      # OAuth client (dev.kick.com) — required if KICK_CHANNEL_NAMES set
KICK_CLIENT_SECRET=xxx
OFFLINE_NOTIFICATIONS=true|false
BRIEFING_MODEL=google:gemini-3.5-flash  # Model for briefing agents (provider:model)
EXTRACTION_MODEL=google:gemini-3.1-flash-lite  # Model for email extraction (parcel + calendar)
GOOGLE_GENERATIVE_AI_API_KEY=xxx        # Required for google: models
ANTHROPIC_API_KEY=xxx                   # Required for anthropic: models
OPENAI_API_KEY=xxx                      # Required for openai: models
TAVILY_API_KEY=tvly-xxx                 # Tavily web search (briefings + recommendations)
BRIEFINGS_PATH=/path/to/briefings       # Folder with .md briefing configs
FASTMAIL_API_TOKEN=xxx                  # Fastmail API token (JMAP email monitoring)
FASTMAIL_APP_PASSWORD=xxx               # Fastmail app password (CalDAV calendar creation)
FASTMAIL_USERNAME=user@fastmail.com     # Fastmail username
PARCEL_API_KEY=xxx                      # Parcel.app API key (enables parcel tracking)
FASTMAIL_CALENDAR_ID=xxx                # Optional: CalDAV calendar ID (auto-discovers default)
TMDB_API_KEY=xxx                        # TMDB (enables recommendations; v3 key or v4 read token)
RECS_SHORTLIST_MODEL=openai:gpt-5.6-luna # Model for recommendation shortlist scoring
RECS_SELECTION_MODEL=openai:gpt-5.6      # Model for recommendation research + final pick
TASTE_REFLECTION_MODEL=openai:gpt-5.6-luna # Model for versioned taste reflection
TASTE_REFLECTION_SCHEDULE=0 0 4 * * 0    # Weekly taste reflection (Sunday 4am)
RECS_SCHEDULE=0 0 17 * * 1,3,5          # Recommendation cron (default Mon/Wed/Fri 5pm)
PUSHOVER_RECS_TOKEN=xxx                 # Optional: override for recommendation notifications
PODCAST_TASTE_PATH=/path/to/taste.md    # Podcast listener profile (enables podcast recs)
PODCAST_RECS_SCHEDULE=0 0 11 * * 1,3,5  # Podcast recs cron (default Mon/Wed/Fri 11am)
PUSHOVER_PODCAST_TOKEN=xxx              # Optional: override for podcast notifications
CASTRO_ACCESS_ID=xxx                    # Optional: Castro device credential UUID (subscriptions + history)
CASTRO_SECRET_KEY=xxx                   # Optional: Castro device HMAC secret
PODCASTINDEX_KEY=xxx                    # Optional: Podcast Index API key (guest-appearance discovery)
PODCASTINDEX_SECRET=xxx                 # Optional: Podcast Index secret (QUOTE in .env — contains #)
PODCAST_VOICE_ROTATION_MAX=12           # Optional: voices person-searched per run (rotates)
PODCAST_MAX_GUEST_PICKS=6               # Optional: Tier-1 guest picks cap per run
PODCAST_TASTE_REFLECTION_MODEL=openai:gpt-5.6-luna # Model for podcast taste reflection
PODCAST_TASTE_REFLECTION_SCHEDULE=0 0 5 * * 0      # Weekly podcast taste reflection (Sunday 5am)
```

## External Dependencies

- **@ai-sdk/google**, **@ai-sdk/anthropic**, **@ai-sdk/openai**: AI provider SDKs (configured via `BRIEFING_MODEL` env var)
- **@micthiesen/mitools**: Logging, Pushover notifications, config, SQLite entities (source at `../mitools`)
- **got**: HTTP client for all outbound requests (Tavily, platform checks, URL fetching)
- **@mozilla/readability**: Firefox Reader View algorithm for extracting article content
- **linkedom**: Lightweight DOM parser (used by Readability, 3x faster than jsdom)
- **turndown** + **turndown-plugin-gfm**: HTML to Markdown conversion with table support
- **node-cron**: Scheduling
- **gray-matter**: YAML frontmatter parsing for briefing config files
- **zod**: Schema validation (config, API responses)
- **html-entities**: Decode HTML entities in YouTube titles
- **vitest**: Testing framework

## Platform-Specific Notes

### YouTube
- Scrapes `/{username}/live` page HTML
- Checks for `ytInitialPlayerResponse` to detect CAPTCHA/consent pages
- Regex matches `"isLive":true` or `"isLiveNow":true`
- Title from `<meta name="title">` tag
- Could break if YouTube changes HTML structure (rare)

### Twitch
- Uses public GQL API: `https://gql.twitch.tv/gql`
- Public client ID: `kimne78kx3ncx6brgo4mv6wki5h1ko` (no auth needed)
- Query: `user(login:"xxx"){stream{title viewersCount}}`
- Zod validates response schema
- More stable than YouTube scraping

### Fastmail Integration (JMAP + CalDAV)

Both parcel-tracker and calendar-events share the same JMAP infrastructure (`src/jmap/`):
- **JMAP** (via `jmap-jam`): Email monitoring via SSE event source
- **EmailDispatcher**: Fetches emails once per state change, fans out to registered `EmailHandler`s (parcel-tracker, calendar-events). Owns the shared JMAP state cursor.
- **CalDAV** (raw HTTP): Calendar event creation — `PUT` iCalendar files to Fastmail's CalDAV endpoint
- Auth: `FASTMAIL_API_TOKEN` (bearer token for JMAP), `FASTMAIL_APP_PASSWORD` (basic auth for CalDAV)
- Each pipeline implements `EmailHandler` with a `handleEmails(emails)` method for filtering and processing
- `jmap-jam` only supports email methods; calendar uses raw `fetch()` against `https://caldav.fastmail.com/dav/calendars/`

## Common Tasks

### Debug a channel status
```bash
# Check Twitch manually
curl -s -X POST 'https://gql.twitch.tv/gql' \
  -H 'Client-Id: kimne78kx3ncx6brgo4mv6wki5h1ko' \
  -H 'Content-Type: application/json' \
  -d '{"query":"query{user(login:\"USERNAME\"){stream{title viewersCount}}}"}'
```

### Reset local state
Delete `docstore.db*` files to clear all channel status/metrics.

## Owner Preferences

- Prefers clean, strongly-typed code
- Likes discriminated unions over boolean flags
- Values robustness (the three-state system was specifically requested)
- Prefers analysis before implementation
- Appreciates GitHub code search for researching approaches
- Wants tests and build to pass before considering work complete
