# Omni Notify

Sends Pushover notifications when YouTube or Twitch channels go live.

## Setup

Use Docker Compose:

```yaml
version: '3.7'
services:
  omni-notify:
    image: ghcr.io/micthiesen/omni-notify:latest
    container_name: omni-notify
    environment:
      - LOG_LEVEL=info
      - PUSHOVER_TOKEN=token
      - PUSHOVER_USER=user
      - YT_CHANNEL_NAMES=@mkbhd:MKBHD,@pewdiepie:PewDiePie
      - TWITCH_CHANNEL_NAMES=shroud:Shroud,xqc:xQc
      - OFFLINE_NOTIFICATIONS=true
    restart: unless-stopped
```

Channel names use the format `username:DisplayName` where the display name is
used in notifications. The display name is optional and defaults to the username
(e.g., `shroud` and `shroud:Shroud` both work, but the latter shows "Shroud" in
notifications).

You can find YouTube channel names from their channel page. Twitch usernames are
found in the channel URL (e.g., `twitch.tv/shroud`).

## Usage

The service will check to see if a channel is live every 20 seconds. It will
send a notification if the channel transitions from offline to live (or vice
versa). The offline notifications can be disabled (see above).

## Stream Filtering (Optional)

You can configure per-channel LLM-based filtering to only receive notifications
for streams that match your interests. This uses Google Gemini Flash to analyze
stream titles and categories.

### Setup

1. Get a [Google AI API key](https://aistudio.google.com/apikey)
2. Set the `GOOGLE_GENERATIVE_AI_API_KEY` environment variable
3. Create a `channels.json` file (or set `CHANNELS_CONFIG_PATH` to a custom path)

### Configuration

Create a `channels.json` file in the project root:

```json
{
  "twitch": {
    "shroud": {
      "filter": {
        "prompt": "I like FPS games and variety content. Skip mobile games and sponsored streams.",
        "defaultOnError": true
      }
    }
  },
  "youtube": {
    "@mkbhd": {
      "filter": {
        "prompt": "Only notify for smartphone and laptop reviews, not studio vlogs.",
        "defaultOnError": false
      }
    }
  }
}
```

- **`prompt`**: Describe what types of streams you want to be notified about
- **`defaultOnError`**: Whether to notify (`true`) or skip (`false`) if the LLM fails

Channels not in the config file will always send notifications (default behavior).

See `channels.example.json` for more examples.

## How it Works

**YouTube**: Scrapes the channel's live page for specific text. This could break
if YouTube changes the page structure or blocks requests (does not use the API).

**Twitch**: Uses Twitch's public GraphQL API to check stream status. No
authentication required.

Statuses are stored in a SQLite database, so they should be remembered between
restarts.

Inspired by: <https://github.com/your-diary/youtube_live_alert>
