# Omni Notify

Monitors YouTube, Twitch, and Kick channels and sends [Pushover](https://pushover.net/) notifications when they go live or offline. Optionally runs AI-powered briefing agents that search the web on a schedule and send notification summaries.

## Quick Start

```yaml
services:
  omni-notify:
    image: ghcr.io/micthiesen/omni-notify:latest
    environment:
      - PUSHOVER_TOKEN=xxx
      - PUSHOVER_USER=xxx
      - YT_CHANNEL_NAMES=@mkbhd:MKBHD,@pewdiepie:PewDiePie
      - TWITCH_CHANNEL_NAMES=shroud:Shroud,xqc:xQc
      - KICK_CHANNEL_NAMES=destiny:Destiny
      - KICK_CLIENT_ID=xxx
      - KICK_CLIENT_SECRET=xxx
    restart: unless-stopped
```

Channel format is `username:DisplayName`. The display name is optional and defaults to the username.

**Same display name = same streamer.** Entries across platforms that share a display name (case-insensitive, whitespace-trimmed) merge into a single streamer. You get **one** "went live" notification when they start streaming on any platform and **one** "went offline" notification when all platforms go offline — no double-pings for multistreams.

## How It Works

Checks every 20 seconds (with random jitter) whether monitored channels are live. Sends a notification on aggregate status transitions (offline-everywhere → live-anywhere, or live-anywhere → offline-everywhere). State is persisted in SQLite so it survives restarts.

- **YouTube**: Scrapes the channel's `/live` page HTML. No API key needed, but could break if YouTube changes its page structure.
- **Twitch**: Uses Twitch's public GraphQL API. No authentication required. More stable than YouTube scraping.
- **Kick**: Uses Kick's official public API (`api.kick.com/public/v1/channels`). Requires registering an app at [dev.kick.com](https://dev.kick.com) and providing `KICK_CLIENT_ID` + `KICK_CLIENT_SECRET` (scope: `channel:read`). The app-only access token is cached and refreshed automatically.

**Primary binding.** When a streamer is live on multiple platforms, one binding is chosen as the "primary" for the notification URL and title. The first platform to go live wins, and sticks for the rest of the session. If they go live simultaneously, priority order is YouTube → Twitch → Kick.

Set `OFFLINE_NOTIFICATIONS=false` to only get notified when channels go live.

## Per-Streamer Overrides

Optionally provide a `channels.json` (or set `CHANNELS_CONFIG_PATH`) to override the Pushover token for specific streamers. The key is the display name (case-insensitive):

```json
{
  "Destiny": {
    "pushoverToken": "app-token-for-destiny"
  }
}
```

## Briefing Agents

AI agents that search the web on a schedule and send notification summaries. Requires `TAVILY_API_KEY`, `BRIEFINGS_PATH`, and an API key for your chosen model provider.

Create `.md` files in your briefings folder:

```markdown
---
schedule: "0 0 8 * * *"
---
You are a morning news assistant. Today is {{date}}, {{time}}.

{{history:10}}

Search for the most important news from the past 24 hours.
Do not cover topics from past notifications above.
```

- Filename becomes the task name (`CanadianNews.md` registers as "CanadianNews")
- `schedule` is a 6-field node-cron expression (with seconds)
- The body is the prompt sent to the AI agent

### Placeholders

| Placeholder | Description | Example |
|---|---|---|
| `{{date}}` | Current date (local timezone) | `Thursday, February 6, 2026` |
| `{{time}}` | Current time (local timezone) | `9:00 AM EST` |
| `{{history:N}}` | Last N notifications from this briefing | _(titles + URLs)_ |

History is stored per-briefing in SQLite and auto-pruned to the last 50 entries.

## Media Recommendations

A scheduled pipeline (default: Mon/Wed/Fri at 5pm) that picks at most one movie or TV title per run, acquires missing titles through Radarr or Sonarr, and sends a Pushover notification explaining the pick. Titles already available in Plex are recommended without another acquisition request.

Each run:

1. Polls Plex history, series-level progress, local availability, and the Radarr/Sonarr tracked catalog. It labels passive outcomes (started, watched, abandoned, ignored) and incorporates explicit "good pick" or "not for me" feedback.
2. Builds a candidate pool from TMDB (recommendations seeded by recent watches, genre discovery, trending, plus a novelty bucket outside your usual genres).
3. Hard-filters in code: anything watched, in progress, tracked by Radarr/Sonarr, explicitly rejected, or recently recommended is dropped before a model sees it.
4. Enriches candidates with structured TMDB commitment and creative metadata, then scores the pool with a cheap model (`RECS_SHORTLIST_MODEL`) and keeps the top 5.
5. Researches the finalists with web search, then a strong model (`RECS_SELECTION_MODEL`) picks exactly one title or decides to add nothing that day.

Plex is the source of watch history, in-progress state, and local availability. Radarr handles movie acquisition and Sonarr handles TV acquisition. All reads fail closed: an unavailable service skips the run instead of treating missing state as an empty library.

A separate weekly `TasteReflection` task maintains a versioned taste profile. It converts Plex observations and recommendation outcomes into an idempotent evidence ledger, computes behavioral statistics, and performs a bounded draft-and-critic reflection. Every learned claim must cite stored evidence. The latest profile is added to recommendation context and shown in the UI; code, prompts, and scoring rules are never self-modified. If no evidence changed, reflection exits without a model call.

See [the recommendation review checkpoint](docs/recommendations-review.md) for the decisions intentionally deferred until enough real recommendations have outcomes.

## Podcast Recommendations

A sibling pipeline (default: Mon/Thu at 11am, enabled by setting `PODCAST_TASTE_PATH`) that recommends up to two fresh podcast **episodes** per run from shows you don't already follow, and sends a Pushover notification per pick.

Each run:

1. Reads subscribed shows and listen history from the Castro account (see [docs/castro-sync.md](docs/castro-sync.md)). Subscribed shows are excluded from recommendations and double as taste evidence alongside the seed profile and explicit feedback; a failed account read aborts the run rather than risk recommending a followed show. Without Castro credentials the pipeline still runs off the seed profile and feedback alone.
2. Discovers episodes being discussed this week via multi-angle web search, then extracts specific candidates with a cheap model.
3. Verifies every candidate in code: show identity via the iTunes Search API, episode and release date from the show's actual RSS feed. Unverifiable episodes are dropped — release dates are never trusted from search snippets.
4. Hard-filters in code: older than 7 days, already recommended, show on 30-day cooldown, rejected via "not for me", or already subscribed.
5. Scores the survivors with a cheap model, researches finalists with web search, then a strong model picks one episode at a time or decides to add nothing.

Recommended episodes are never repeated. When Castro credentials are set, listen history labels outcomes (listened / abandoned / ignored) automatically; the good-pick/not-for-me feedback buttons in the web UI are always available.

The same Castro credentials enable an independent `CastroQueueCleanup` task.
It runs hourly and silently clears queued episodes whose description begins
with `This is a free preview`, the standard marker used by Substack preview
episodes. Matching is deliberately case-sensitive and prefix-only.

## Web UI

The built-in server (port `FRONTEND_PORT`, default 3000) serves the Omni Notify dashboard:

- `/` shows live streamer status (who's live now, title, uptime, peak viewers), a stat strip, every scheduled task with its cron schedule, ticking next-run countdown, "Run now" button and expandable run history, plus a recent-activity feed with per-task filtering.
- `/pets` is the pet weight tracker.
- `/recommendations` lists every recommendation with poster, status, reasoning, service links, explicit feedback controls, filters, the current evidence-backed taste profile, and recent pipeline activity. Pushover notifications deep-link to the relevant recommendation.
- `/podcasts` lists podcast episode recommendations with show artwork, status filters, episode/discussion links, and good-pick/not-for-me feedback controls. Pushover notifications deep-link here too.

Updates are pushed in realtime over SSE (`/api/events`) on the same HTTP port — no extra ports needed; the UI falls back to polling `/api/snapshot` (and shows a "Reconnecting" badge) if the stream drops. Task runs are persisted in SQLite (last 50 per task) so history survives restarts.

To iterate on the frontend without real credentials, `src/tools/preview-server.ts` boots the real server with fake tasks, streamers, runs, recommendations, and a pet:

```bash
DB_NAME=/tmp/omni-preview.db FRONTEND_PORT=3999 npx tsx src/tools/preview-server.ts
```

## AI Model Configuration

Models are configured via environment variables using `provider:model` format. Supported providers: `google`, `anthropic`, `openai`. You only need an API key for the provider you're using.

| Variable | Default | Used for |
|---|---|---|
| `BRIEFING_MODEL` | `google:gemini-3.5-flash` | Briefing agents |
| `EXTRACTION_MODEL` | `google:gemini-3.1-flash-lite` | Email extraction (parcel + calendar) |
| `RECS_SHORTLIST_MODEL` | `openai:gpt-5.6-luna` | Recommendation shortlist scoring |
| `RECS_SELECTION_MODEL` | `openai:gpt-5.6` | Recommendation research + final pick |
| `TASTE_REFLECTION_MODEL` | `openai:gpt-5.6-luna` | Weekly evidence-backed taste reflection |

Examples:

```bash
BRIEFING_MODEL=google:gemini-3.5-flash
BRIEFING_MODEL=anthropic:claude-sonnet-5
BRIEFING_MODEL=openai:gpt-5.6
```

## Environment Variables

| Variable | Required | Description |
|---|---|---|
| `PUSHOVER_USER` | Yes | Pushover user key |
| `PUSHOVER_TOKEN` | Yes | Pushover app token |
| `YT_CHANNEL_NAMES` | No | YouTube channels (`@handle:Name,...`) |
| `TWITCH_CHANNEL_NAMES` | No | Twitch channels (`username:Name,...`) |
| `KICK_CHANNEL_NAMES` | No | Kick channels (`slug:Name,...`). Requires `KICK_CLIENT_ID`/`KICK_CLIENT_SECRET`. |
| `KICK_CLIENT_ID` | No | Kick OAuth client ID ([dev.kick.com](https://dev.kick.com)) |
| `KICK_CLIENT_SECRET` | No | Kick OAuth client secret |
| `OFFLINE_NOTIFICATIONS` | No | Send offline notifications (default: `true`) |
| `BRIEFING_MODEL` | No | AI model for briefings (default: `google:gemini-3.5-flash`) |
| `EXTRACTION_MODEL` | No | AI model for email extraction (default: `google:gemini-3.1-flash-lite`) |
| `GOOGLE_GENERATIVE_AI_API_KEY` | No | Required for `google:` models |
| `ANTHROPIC_API_KEY` | No | Required for `anthropic:` models |
| `OPENAI_API_KEY` | No | Required for `openai:` models |
| `TAVILY_API_KEY` | No | Tavily web search (required for briefings + recommendations) |
| `BRIEFINGS_PATH` | No | Folder containing `.md` briefing configs |
| `CHANNELS_CONFIG_PATH` | No | Path to `channels.json` for per-streamer overrides |
| `TMDB_API_KEY` | No | TMDB API key (required for recommendations; v3 key or v4 read token) |
| `RECS_SCHEDULE` | No | Recommendation cron (default: `0 0 17 * * 1,3,5`) |
| `TASTE_REFLECTION_MODEL` | No | Model for evidence-backed taste reflection (default: `openai:gpt-5.6-luna`) |
| `TASTE_REFLECTION_SCHEDULE` | No | Taste-profile reflection cron (default: `0 0 4 * * 0`, Sunday 4am) |
| `RECS_PUBLIC_URL` | No | Public/LAN Omni base URL used by notification links (default: `http://omni.boris`) |
| `PUSHOVER_RECS_TOKEN` | No | Pushover token for recommendations (falls back to `PUSHOVER_TOKEN`) |
| `PODCAST_TASTE_PATH` | No | Markdown listener profile (required to enable podcast recommendations) |
| `PODCAST_RECS_SCHEDULE` | No | Podcast recommendation cron (default: `0 0 11 * * 1,4`) |
| `PUSHOVER_PODCAST_TOKEN` | No | Pushover token for podcast recs (falls back to `PUSHOVER_TOKEN`) |
| `CASTRO_ACCESS_ID` / `CASTRO_SECRET_KEY` | No | Castro device credentials (account reads, queue writes, and hourly preview cleanup) |
| `PLEX_URL` / `PLEX_TOKEN` | For recommendations | Plex server URL and token |
| `PLEX_ACCOUNT_ID` | For shared Plex servers | Account ID used to scope viewing history; multiple detected accounts fail closed without it |
| `RADARR_URL` / `RADARR_API_KEY` | For recommendations | Radarr v3 API connection |
| `RADARR_ROOT_FOLDER_PATH` / `RADARR_QUALITY_PROFILE_ID` | For recommendations | Defaults for acquired movies |
| `SONARR_URL` / `SONARR_API_KEY` | For recommendations | Sonarr v3 API connection |
| `SONARR_ROOT_FOLDER_PATH` / `SONARR_QUALITY_PROFILE_ID` | For recommendations | Defaults for acquired series |
| `LOG_LEVEL` | No | `debug`, `info`, `warn`, or `error` |

## Development

```bash
pnpm dev        # Development with hot reload
pnpm build      # TypeScript compilation
pnpm test       # Run tests (vitest)
pnpm check      # Biome linting + formatting
```

Inspired by [youtube_live_alert](https://github.com/your-diary/youtube_live_alert).
