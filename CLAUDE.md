# CLAUDE.md

> This is a living document. Update it when you learn new preferences, patterns, or project conventions. Don't ask—just update it if something is missing or outdated.

## Project Overview

**omni-notify** monitors livestream platforms (YouTube, Twitch) and sends Pushover notifications when channels go live or offline. Runs on a 20-second cron schedule with random jitter.

## Quick Reference

```bash
bun run dev      # Development with hot reload
bun run build    # TypeScript compilation (run after changes)
bun test         # Run tests
bun run check    # Biome linting + formatting check
```

**Always run `bun test && bun run build` after making changes.**

## Architecture

```
src/
├── index.ts                 # Entry point, cron setup
├── platforms/               # Platform implementations
│   ├── index.ts             # Platform enum, types, config registry
│   ├── common.ts            # Shared fetch utilities (fetchPageHtml, fetchGQL)
│   ├── youtube.ts           # YouTube HTML scraping
│   ├── twitch.ts            # Twitch GQL API
│   └── *.spec.ts            # Platform tests
├── tasks/
│   ├── types.ts             # Abstract Task class
│   ├── TaskManager.ts       # Task orchestration
│   ├── LiveCheckTask.ts     # Main logic: status transitions, notifications
│   └── persistence/         # SQLite via @micthiesen/mitools Entity
│       ├── status.ts        # Channel live/offline state
│       └── metrics.ts       # All-time stats (max viewers)
└── utils/
    └── config.ts            # Environment config with zod validation
```

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

1. Create `src/platforms/{platform}.ts` with:
   - `fetch{Platform}LiveStatus({ username })` → `Promise<FetchedStatus>`
   - `get{Platform}LiveUrl(username)` → `string`
   - `extractLiveStatus(data)` → `FetchedStatus` (for testing)

2. Update `src/platforms/index.ts`:
   - Add to `Platform` enum
   - Add to `platformConfigs` record

3. Update `src/utils/config.ts`:
   - Add `{PLATFORM}_CHANNEL_NAMES: commaSeparatedString`

4. Update `src/tasks/TaskManager.ts`:
   - Add to `LiveCheckTask` constructor array

5. Update `.env.example` and `README.md`

6. Create `src/platforms/{platform}.spec.ts`

### Error Handling

- Providers catch their own errors and return `LiveStatus.Unknown`
- `LiveCheckTask` tracks consecutive unknowns per channel
- Escalating log levels: debug (1-2) → warn (3-9) → error (10+)
- Logger `warn`/`error` go to Pushover via @micthiesen/mitools

### Persistence

Uses `@micthiesen/mitools` Entity system with SQLite (`docstore.db`):
- `ChannelStatusEntity`: Current live/offline state, timestamps, max viewers per stream
- `ChannelMetricsEntity`: All-time stats per channel

## Code Style

- **Biome** for formatting (88 char line width, 2-space indent) and linting
- **2 spaces** for indentation (not tabs)
- **Strong types**: Use enums, discriminated unions, explicit return types
- **No over-engineering**: Simple solutions, no unnecessary abstractions
- **Clean code**: No debug leftovers, no `console.log`, no commented code

## Testing

```bash
bun test              # Run all tests
bun test --watch      # Watch mode
```

- YouTube tests are skipped (marked `.skip`) - they need real HTML fixtures
- Twitch tests run against `extractLiveStatus` with mock data
- Test the pure extraction functions, not the network fetchers

## Environment Variables

```bash
LOG_LEVEL=info|debug|warn|error
PUSHOVER_USER=xxx
PUSHOVER_TOKEN=xxx
YT_CHANNEL_NAMES=@channel1,@channel2    # YouTube handles
TWITCH_CHANNEL_NAMES=user1,user2        # Twitch usernames
OFFLINE_NOTIFICATIONS=true|false
```

## External Dependencies

- **@micthiesen/mitools**: Logging, Pushover notifications, config, SQLite entities
- **node-cron**: Scheduling
- **zod**: Schema validation (config, API responses)
- **html-entities**: Decode HTML entities in YouTube titles

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
