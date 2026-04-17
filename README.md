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

## AI Model Configuration

Models are configured via environment variables using `provider:model` format. Supported providers: `google`, `anthropic`, `openai`. You only need an API key for the provider you're using.

| Variable | Default | Used for |
|---|---|---|
| `BRIEFING_MODEL` | `google:gemini-3-pro-preview` | Briefing agents |

Examples:

```bash
BRIEFING_MODEL=google:gemini-3-pro-preview
BRIEFING_MODEL=anthropic:claude-sonnet-4
BRIEFING_MODEL=openai:gpt-4.1
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
| `BRIEFING_MODEL` | No | AI model for briefings (default: `google:gemini-3-pro-preview`) |
| `GOOGLE_GENERATIVE_AI_API_KEY` | No | Required for `google:` models |
| `ANTHROPIC_API_KEY` | No | Required for `anthropic:` models |
| `OPENAI_API_KEY` | No | Required for `openai:` models |
| `TAVILY_API_KEY` | No | Tavily web search (required for briefings) |
| `BRIEFINGS_PATH` | No | Folder containing `.md` briefing configs |
| `CHANNELS_CONFIG_PATH` | No | Path to `channels.json` for per-streamer overrides |
| `LOG_LEVEL` | No | `debug`, `info`, `warn`, or `error` |

## Development

```bash
pnpm dev        # Development with hot reload
pnpm build      # TypeScript compilation
pnpm test       # Run tests (vitest)
pnpm check      # Biome linting + formatting
```

Inspired by [youtube_live_alert](https://github.com/your-diary/youtube_live_alert).
