# Omni Notify

Monitors YouTube and Twitch channels and sends [Pushover](https://pushover.net/) notifications when they go live or offline. Optionally runs AI-powered briefing agents that search the web on a schedule and send notification summaries.

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
    restart: unless-stopped
```

Channel format is `username:DisplayName`. The display name is optional and defaults to the username.

## How It Works

Checks every 20 seconds (with random jitter) whether monitored channels are live. Sends a notification on status transitions (offline to live, or live to offline). State is persisted in SQLite so it survives restarts.

- **YouTube**: Scrapes the channel's `/live` page HTML. No API key needed, but could break if YouTube changes its page structure.
- **Twitch**: Uses Twitch's public GraphQL API. No authentication required. More stable than YouTube scraping.

Set `OFFLINE_NOTIFICATIONS=false` to only get notified when channels go live.

## Stream Filtering

Per-channel LLM-based filtering to only get notified for streams matching your interests. Create a `channels.json` (or set `CHANNELS_CONFIG_PATH`):

```json
{
  "twitch": {
    "shroud": {
      "filter": {
        "prompt": "I like FPS games. Skip mobile games and sponsored streams.",
        "defaultOnError": true
      }
    }
  }
}
```

- **`prompt`**: Describe what streams you care about
- **`defaultOnError`**: Whether to notify (`true`) or skip (`false`) if the LLM call fails

Channels without a filter always send notifications.

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
| `FILTER_MODEL` | `google:gemini-3-flash-preview` | Stream notification filters |

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
| `OFFLINE_NOTIFICATIONS` | No | Send offline notifications (default: `true`) |
| `BRIEFING_MODEL` | No | AI model for briefings (default: `google:gemini-3-pro-preview`) |
| `FILTER_MODEL` | No | AI model for stream filters (default: `google:gemini-3-flash-preview`) |
| `GOOGLE_GENERATIVE_AI_API_KEY` | No | Required for `google:` models |
| `ANTHROPIC_API_KEY` | No | Required for `anthropic:` models |
| `OPENAI_API_KEY` | No | Required for `openai:` models |
| `TAVILY_API_KEY` | No | Tavily web search (required for briefings) |
| `BRIEFINGS_PATH` | No | Folder containing `.md` briefing configs |
| `CHANNELS_CONFIG_PATH` | No | Path to `channels.json` for stream filters |
| `LOG_LEVEL` | No | `debug`, `info`, `warn`, or `error` |

## Development

```bash
pnpm dev        # Development with hot reload
pnpm build      # TypeScript compilation
pnpm test       # Run tests (vitest)
pnpm check      # Biome linting + formatting
```

Inspired by [youtube_live_alert](https://github.com/your-diary/youtube_live_alert).
