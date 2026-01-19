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
      - YT_CHANNEL_NAMES=@some,@channel,@usernames
      - TWITCH_CHANNEL_NAMES=shroud,xqc
      - OFFLINE_NOTIFICATIONS=true
    restart: unless-stopped
```

You can find YouTube channel names from their channel page. Twitch usernames are
found in the channel URL (e.g., `twitch.tv/shroud`).

## Usage

The service will check to see if a channel is live every 20 seconds. It will
send a notification if the channel transitions from offline to live (or vice
versa). The offline notifications can be disabled (see above).

## How it Works

**YouTube**: Scrapes the channel's live page for specific text. This could break
if YouTube changes the page structure or blocks requests (does not use the API).

**Twitch**: Uses Twitch's public GraphQL API to check stream status. No
authentication required.

Statuses are stored in a SQLite database, so they should be remembered between
restarts.

Inspired by: <https://github.com/your-diary/youtube_live_alert>
