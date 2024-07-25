# Omni Notify

Currently only supports sending a Pushover notification when a YouTube channel goes live.

## Setup

Use Docker Compose:

```yaml
version: '3.7'
services:
  omni-notify:
    image: ghcr.io/micthiesen/omni-notify:latest
    container_name: omni-notify
    environment:
      - PUSHOVER_APP_TOKEN=token
      - PUSHOVER_USER_KEY=key
      - YT_CHANNEL_NAMES=@some,@channel,@usernames
    restart: unless-stopped
```

You can find YouTube channel names from their channel page.

## Usage

The service will check to see if a channel is live every 20 seconds. It will only send a notification if the
channel changes from NOT live to live (or at startup).

## How it Works

It looks for specific text on the channel's live page. Because of this, it could break if YouTube changes what
the page looks like or if YouTube blocks the requests for some reason (it does not use the API).

Inspired by: <https://github.com/your-diary/youtube_live_alert>
