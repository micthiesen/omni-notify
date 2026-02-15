# CLAUDE.md

> This is a living document. Update it when you learn new preferences, patterns, or project conventions. Don't ask—just update it if something is missing or outdated.

## Project Overview

**omni-notify** monitors livestream platforms (YouTube, Twitch) and sends Pushover notifications when channels go live or offline. Runs on a 20-second cron schedule with random jitter.

## Quick Reference

```bash
pnpm dev      # Development with hot reload
pnpm build    # TypeScript compilation (run after changes)
pnpm test     # Run tests (vitest)
pnpm check    # Biome linting + formatting check
```

**Always run `pnpm test && pnpm build` after making changes.**

## Architecture

```
src/
├── index.ts                 # Entry point, registers scheduled tasks
├── scheduling/              # Generic scheduling infrastructure
│   ├── Scheduler.ts         # Cron management, graceful shutdown
│   └── ScheduledTask.ts     # Abstract base class for tasks
├── live-check/              # Livestream monitoring feature
│   ├── task.ts              # LiveCheckTask: status transitions, notifications
│   ├── persistence.ts       # Channel live/offline state (SQLite)
│   ├── platforms/           # Platform implementations
│   │   ├── index.ts         # Platform enum, types, config registry
│   │   ├── common.ts        # Shared fetch utilities
│   │   ├── youtube.ts       # YouTube HTML scraping
│   │   └── twitch.ts        # Twitch GQL API
│   ├── metrics/             # Viewer metrics with rolling windows
│   │   ├── ViewerMetricsService.ts  # Peak confirmation state machine
│   │   ├── persistence.ts   # ViewerMetricsEntity (daily buckets)
│   │   └── windows.ts       # Rolling window calculation helpers
│   └── filters/             # Stream notification filtering
├── ai/                      # AI model configuration
│   └── registry.ts          # Provider registry (Google, Anthropic, OpenAI)
├── briefing-agent/          # AI-powered briefing tasks (web search → notify)
│   ├── BriefingAgentTask.ts # Config-driven task class
│   └── configs.ts           # Loads briefing configs from BRIEFINGS_PATH .md files
├── tools/                   # Shared AI agent tools (reusable across any agent)
│   ├── webSearch.ts         # Tavily web search tool
│   └── fetchUrl.ts          # URL fetcher: HTML → clean markdown via Readability + Turndown
├── emails/                  # Email utilities (general purpose)
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
   - Add to `channels` array passed to `LiveCheckTask`

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

### Persistence

Uses `@micthiesen/mitools` Entity system with SQLite (`docstore.db`):
- `ChannelStatusEntity`: Current live/offline state, timestamps, max viewers per stream
- `ViewerMetricsEntity`: Daily viewer buckets for rolling window calculations, all-time max

### Viewer Metrics System

Tracks viewer records across rolling time windows (7d, 30d, 90d, all-time) using **peak confirmation**:
- Records are only confirmed when viewer count drops 5% below peak (prevents spam during climbs)
- Pending peaks are flushed when stream goes offline
- Only sends one notification for the highest-priority window when multiple records are broken

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
PUSHOVER_TOKEN=xxx
YT_CHANNEL_NAMES=@channel1,@channel2    # YouTube handles
TWITCH_CHANNEL_NAMES=user1,user2        # Twitch usernames
OFFLINE_NOTIFICATIONS=true|false
BRIEFING_MODEL=google:gemini-3-pro      # Model for briefing agents (provider:model)
FILTER_MODEL=google:gemini-3-flash      # Model for stream notification filters
GOOGLE_GENERATIVE_AI_API_KEY=xxx        # Required for google: models
ANTHROPIC_API_KEY=xxx                   # Required for anthropic: models
OPENAI_API_KEY=xxx                      # Required for openai: models
TAVILY_API_KEY=tvly-xxx                 # Tavily web search (for briefing agents)
BRIEFINGS_PATH=/path/to/briefings       # Folder with .md briefing configs
```

## External Dependencies

- **@ai-sdk/google**, **@ai-sdk/anthropic**, **@ai-sdk/openai**: AI provider SDKs (configured via `BRIEFING_MODEL` env var)
- **@micthiesen/mitools**: Logging, Pushover notifications, config, SQLite entities
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
