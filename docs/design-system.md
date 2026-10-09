# Omni design system: Instrument

This is the reference for the Omni web UI: the Leptos SPA in `crates/omni-web`,
`crates/omni-web-kit` and `crates/omni-web-pages`, and the server-rendered
`/reminders` page in `crates/omni-reminders`. It replaces the earlier styling
completely. Class names may change; routes, deep links, ids listed in section 13
and live-update behavior may not.

The system is direction A ("Instrument"), with grafts from B ("Nightfall") and
C ("Console"). The reasons are in the design decision record kept with the
redesign work. Grafts are marked *(from B)* or *(from C)* where they matter.

## 1. Principles

1. **Numbers are the interface.** Viewer counts, countdowns, costs and durations
   are mono readouts with tabular figures. A number that matters is never smaller
   than its label.
2. **Color means deviation.** Surfaces are graphite and text is neutral. Success
   is a quiet dot without a word. Hue and words appear only for live, running,
   warning, failure and "needs you". The one accent, **Signal** (lime), means
   "you are here", "primary action" or "this just changed". It never means "OK".
3. **One hero per page.** Each page leads with exactly one focal element: a
   status sentence, a live readout, a player or a task table. No eyebrow, title,
   subtitle and card stack.
4. **Rows and hairlines before boxes.** Panels hold groups of rows. Cards exist
   only for things with imagery (posters, artwork) and for floating layers.
5. **Light comes from live things and artwork** *(from B)*. The only warm glow in
   the app is behind something on air, and the only saturated imagery is real
   posters and artwork. A calm system looks nearly colorless.
6. **Same job, same control.** One button family, one segmented control, one
   disclosure, one inspector, one two-step confirm, one toast region, everywhere.
7. **Keyboard reaches everything; touch loses nothing** *(from C)*. Every
   shortcut has a visible 44 px touch equivalent.
8. **Fast by default.** No new crates or JS, no blur filters on scrolling
   content, transform/opacity animations only, show-more instead of
   virtualization.

## 2. Tokens

All tokens live on `:root` in `style/tokens.css`. The app is dark only:
`color-scheme: dark`. No raw `rgba()` or hex values outside the tokens file
(SVG presentation attributes reference tokens through `var(--…)`). The one
exception is `#000` inside `mask-image` gradients, where only alpha matters.

### 2.1 Surfaces and lines

| Token | Value | Role |
|---|---|---|
| `--canvas` | `#0a0b0d` | Page background, rail, top bar, `theme-color` |
| `--surface` | `#111316` | Panels, table bodies, inputs at rest |
| `--raised` | `#171a1e` | Selected nav item, active segment, secondary button |
| `--hover` | `#1a1d22` | Row and nav hover wash |
| `--overlay` | `#1d2126` | Popovers, tooltips, palette, toasts, sheets, inspector |
| `--well` | `#07080a` | Log wells, JSON blocks, code |
| `--line` | `#22262c` | Hairlines between rows, panel borders |
| `--line-strong` | `#30353d` | Control borders, overlay borders |
| `--scrim` | `rgb(5 6 8 / .62)` | Behind sheets, drawers and the palette |

### 2.2 Text

| Token | Value | Contrast (canvas / surface / overlay) | Role |
|---|---|---|---|
| `--text` | `#eceef1` | 16.9 / 16.0 / 13.9 | Primary text, readouts |
| `--text-2` | `#a9b0ba` | 9.0 / 8.5 / 7.4 | Body copy, secondary values |
| `--text-3` | `#858d98` | 5.9 / 5.6 / 4.8 | Labels, meta, axis text (floor for readable text) |
| `--text-off` | `#4b525b` | 2.5 | Disabled glyphs, separators, decoration only |

### 2.3 Signal (the one accent)

| Token | Value | Role |
|---|---|---|
| `--signal` | `#c6f36a` | Focus ring, current-nav tick, primary button, selected data point, running task name, value-changed flash (15.4:1 on canvas) |
| `--signal-hi` | `#d8fa94` | Primary button hover |
| `--signal-ink` | `#0c1004` | Text on Signal (15.1:1) |
| `--signal-wash` | `rgb(198 243 106 / .10)` | Selected row fill, pressed chip |
| `--signal-line` | `rgb(198 243 106 / .32)` | Selected outlines |

At most one Signal-filled button per view.

### 2.4 Semantic hues (deviation only)

Each hue has `-wash` (`/ .11`) and `-line` (`/ .34`) variants built from the same
`rgb()` channels. These replace all hand-written tints.

| Token | Value | Meaning | Shape (never color alone) |
|---|---|---|---|
| `--live` | `#ff4d6a` | On air | Pulsing round dot plus the word LIVE |
| `--ok` | `#4ade9a` | Healthy | Small round dot, no word |
| `--warn` | `#f4b740` | Degraded, skipped, stale, polling, armed confirm | Diamond plus a word |
| `--fault` | `#ff7452` | Failed, error, destructive | Square plus a word |
| `--info` | `#7ab8ff` | Queued, informational | Ring |

Live (crimson) and fault (vermilion) are both warm, so they are always separated
by shape (pulse against square) and by a word. Live pulses; fault never moves.

Records use Signal plus a `★` glyph, not a separate gold.

### 2.5 Platform marks

`--yt #ff0033`, `--twitch #a970ff`, `--kick #53fc18`, `--dgg #3fa9f5`. Used only
to fill the 16 px platform glyph (`PlatformIcon`) or a 3×12 px tick. Never on text
or surfaces.

### 2.6 Chart colors

| Token | Value | Use |
|---|---|---|
| `--bar` | `#3a414a` | Default single-series bars |
| `--bar-hi` | `#59626e` | Hovered bar, brush handles |
| `--grid` | `#1b1f24` | Gridlines (solid hairline) |
| `--axis` | `var(--text-3)` | Axis labels |
| `--series-1` … `--series-8` | `#3987e5 #d95926 #199e70 #c98500 #d55181 #008300 #9085e9 #e66767` | Multi-series only (costs by feature) |
| `--series-other` | `#5b636e` | Folded "Other" series |

The series palette was run through the dataviz validator against `#111316`
(dark): lightness band, chroma, CVD separation (worst adjacent ΔE 8.4),
normal-vision floor (19.3) and 3:1 contrast all pass. Assign slots in fixed order
by entity, never by rank, never cycled. Prefer slots 1 to 7 and fold the rest into
Other, so slot 8 (red) rarely sits next to a fault mark. Signal and the semantic
hues are never series colors.

### 2.7 Space, size, radius

- Space (4 px base): `--sp-1 4`, `--sp-2 8`, `--sp-3 12`, `--sp-4 16`,
  `--sp-5 20`, `--sp-6 24`, `--sp-8 32`, `--sp-10 40`, `--sp-12 48`, `--sp-16 64`.
  No other padding or gap values.
- Rows: `--row 52px` (desktop default), `--row-dense 36px` (tables in Data, MCP,
  run history), `--row-touch 56px` (phone). Panel padding 20 px desktop, 16 px
  phone.
- Controls: `--ctl-md 36px`, `--ctl-sm 30px`; on phone 44 and 36.
- Radii: `--r-xs 4` (kbd, tags, LIVE tag), `--r-sm 6` (chips, segments),
  `--r-md 8` (buttons, inputs), `--r-lg 12` (panels, posters), `--r-xl 16`
  (sheets, palette, inspector, hero stage). `--r-pill 999px` only for count
  badges and avatars.

### 2.8 Elevation and layers

- `--e-0`: none (panels differ by tier and hairline).
- `--e-1`: `inset 0 1px 0 rgb(255 255 255 / .03)` (panels, so they read machined).
- `--e-2`: `--e-1` plus `0 8px 24px rgb(0 0 0 / .45)` (active segment, popover,
  tooltip).
- `--e-3`: `--e-1` plus `0 24px 64px rgb(0 0 0 / .6)` (inspector, palette, toast,
  sheet).
- Shadows appear only on floating layers.
- Z scale (only these): `--z-sticky 10`, `--z-nav 20`, `--z-drawer 30`,
  `--z-overlay 40`, `--z-toast 50`.

### 2.9 Motion

- `--ease cubic-bezier(.2,.8,.2,1)`; `--d-1 120ms` (hover, press),
  `--d-2 180ms` (disclosure chevron, segment), `--d-3 260ms` (drawer, sheet,
  palette, toast).
- See section 9 for what moves.

## 3. Typography

### 3.1 Fonts

- **Geist** (UI and prose) and **Geist Mono** (readouts, ids, cron, times, logs,
  JSON). Both are SIL Open Font License 1.1.
- **Where to obtain:** the official repository `github.com/vercel/geist-font`.
  Take the variable-weight `woff2` files for Geist and Geist Mono from its latest
  tagged release (the same files ship in the `geist` npm package under
  `dist/fonts/`), plus the repository's `OFL.txt`. Do not use copies pulled from
  other apps' bundles.
- **Vendoring:** `crates/omni-web/assets/fonts/Geist-Variable.woff2`,
  `GeistMono-Variable.woff2` (the official Latin variable webfonts, not further
  subset), their `OFL.txt`, and a `README.md` recording source and license.
  `index.html` copies the directory with
  `<link data-trunk rel="copy-dir" href="assets/fonts">` and preloads both with
  `crossorigin`; `tokens.css` declares `@font-face { font-weight: 100 900;
  font-display: swap }`. Copied files are not content-hashed, so a font update
  renames the file and both references. No build-time tool is added.
- **Fallback stacks:** `Geist, ui-sans-serif, system-ui, -apple-system,
  "Segoe UI", sans-serif` and `"Geist Mono", ui-monospace, "SF Mono", Menlo,
  monospace`.
- Features: `font-feature-settings: "ss01", "cv11"` on body; utility `.num` sets
  `font-variant-numeric: tabular-nums slashed-zero` and is applied to every
  number.

### 3.2 Scale (11 styles)

| Style | Desktop | Phone (≤ 899 px) | Use |
|---|---|---|---|
| `display` | Geist 600 40/44, -0.035em | 32/36 | Status sentence, streamer name |
| `h1` | 600 28/34, -0.02em | 24/30 | Page titles |
| `h2` | 600 18/24, -0.01em | 18/24 | Section titles |
| `h3` | 600 15/20 | 16/22 | Panel titles, row primary text |
| `body` | 400 14/22 | 15/23 | Copy, row secondary text |
| `small` | 400 13/19 | 13/19 | Meta, secondary rows |
| `label` | 500 12/16, sentence case | 12/16 | Column headers, readout keys, group headers |
| `read-xl` | Mono 500 56/56, -0.05em | 44/44 | The one live hero figure per page |
| `read-l` | Mono 500 32/32, -0.03em | 28/28 | Readout band figures |
| `read-m` | Mono 500 20/22 | 20/22 | Row values (viewers, peaks, costs) |
| `read-s` | Mono 450 13/17 | 13/17 | Times, durations, ids, countdowns, cron |

Rules: nothing below 12 px; phone body is 15 px and inputs are 16 px (no iOS
zoom). Uppercase only for the `LIVE` tag and poster kind labels. Long-form
prose (artifacts, transcripts, briefings) is 15/24 with a 68ch measure.

## 4. Layout

### 4.1 Breakpoints (three, mobile-last)

| Name | Range | Shell |
|---|---|---|
| phone | ≤ 899 px | Top bar plus bottom tab bar; sheets instead of drawers |
| desk | 900 to 1279 px | Rail plus single-column content; two-column page layouts stack |
| wide | ≥ 1280 px | Rail plus two-column page layouts; Operations inspector can dock |

Co-locate each component's breakpoint rules with the component. No other
breakpoints.

### 4.2 Desktop shell

```
┌ rail 232 ───────┬ top bar 56 (sticky): crumbs ··············· page actions ┐
│ ◉ Omni          │                                                          │
│ [⌕ Search  ⌘K]  │  content: max 1320 px, 40 px gutters, 12-col grid         │
│ Home            │  (Data, MCP, Operations may go full width)               │
│ On air      4   │                                                          │
│  ● ESLCS 39.2k  │                                                          │
│  ● Hutch 1,732  │                                                          │
│ Watch …         │                                                          │
│ ● Live · 2s     │                                                          │
└─────────────────┴──────────────────────────────────────────────────────────┘
```

- Rail: 232 px, sticky, full height, `--canvas` with a right hairline.
- Top bar: 56 px, sticky, `--canvas` at 88% opacity with a 1 px bottom hairline.
  The only backdrop blur in the app (8 px), because it is small and fixed.
- The rail can collapse to a 64 px icon rail with `[` *(from C)*; the choice is a
  per-viewer `localStorage` preference wrapped in try/catch.

### 4.3 Phone shell

- Top bar 52 px plus safe area: page title (crumbs hidden), connection dot,
  44 px search button. Nothing else.
- Bottom tab bar 64 px plus safe area, `--canvas` with a top hairline.
- Content gutters 16 px; content has bottom padding of tab bar height plus 24 px.
- Sheets replace drawers and modals: bottom sheet up to 88dvh, 16 px top radius,
  grab handle, scrim, Escape and focus trap through `use_modal`.
- Horizontal snap rails for posters (about 2.3 items visible). Tables collapse
  to two-line rows; secondary columns hide, never wrap.

## 5. Navigation

### 5.1 Information architecture

Every destination is visible in the rail, grouped by purpose. There is no "More"
and no sub-tab strip (`SectionNav` is removed).

| Group | Items (route) | Rail badge |
|---|---|---|
| (top) | Home (`/`) | |
| **On air** *(from C)* | Header links to `/live`; below it one row per live streamer: platform tick, name, `read-s` viewer count updated in place, linking to `/streamers/:id`. When nobody is live: "Nobody live" in `--text-3`. | live count in `--live` |
| **Watch** | Movies & TV (`/media`) | pending picks |
| **Listen** | Podcasts (`/podcasts`), PressPods (`/pods`) | jobs in progress |
| **Research** | Workspaces (`/workspaces`), Briefings (`/briefings`) | pending actions |
| **Personal** | Email (`/emails`), Reminders (`/reminders`, full page load), Pets (`/pets`) | email failures (fault) |
| **System** | Operations (`/operations`), Costs (`/costs`), MCP (`/mcp-activity`), Claude (`/claude`), Data (`/data`) | failing tasks (fault) |

- `/live` is a **new route**: the full streamer roster (section 12). Add it to
  `routes.rs` with tests; every existing route, alias and normalization rule is
  unchanged.
- Current item: `--raised` fill, 2 px Signal tick on the left edge,
  `aria-current="page"`. Streamer pages mark their On air row (or Live, when
  offline) as current.
- Rail footer: connection state (section 8.1). Clicking it reloads when not live,
  as today.

### 5.2 Top bar

- Breadcrumbs replace `BackLink`: `Live / Hutch / Intelligence`,
  `Research / Marketplace Selling / 2022 NCM C7…`, `System / Operations`.
  Middle crumbs are links; the last is the page. Long crumbs truncate in the
  middle crumb first.
- Right side: the page's own actions (at most one primary), then on wide screens
  the mono clock `12:14 PDT` *(from C)*.

### 5.3 Phone tab bar

Five tabs, 44 px minimum targets, icon over a 12 px label:

1. **Home** `/`
2. **Live** `/live` (count badge in `--live`)
3. **Media**: lands on the last used of `/media`, `/podcasts`, `/pods` (remembered
   per viewer); those pages show a segmented control `Movies & TV | Podcasts |
   PressPods` under the title on phone only.
4. **Research**: `/workspaces`, with `Workspaces | Briefings` segmented on phone.
5. **Go**: opens the command palette as a sheet listing every destination grouped
   as in the rail, with search on top. It carries the fault badge for failing
   tasks and email failures. This replaces both the More sheet and the floating
   "…".

The active tab gets a Signal 2 px top tick and `--text`; inactive tabs are
`--text-3`. The tab bar hides on `/feedback/*`.

### 5.4 Command palette

- Opens with ⌘K / Ctrl-K, `/` (when no page filter has focus), the rail search
  field, the phone search button and the Go tab.
- Sources, all from data the SPA already loads: routes; streamers from the
  snapshot (live first, with counts); tasks from the snapshot; workspace subjects
  from `/api/workspaces`. No new endpoint.
- Groups: Live now, Pages, Streamers, Tasks, Research. Choosing a task opens it in
  the Operations inspector (`/operations#inspect=<Task>`); the palette never runs
  anything directly.
- `role="dialog"` with a `listbox`; arrow keys move `aria-selected`; Enter opens;
  Escape closes and restores focus. Empty state "No matches for "…"".
- Desktop: centered 640 px modal at 15vh. Phone: full-height sheet with the input
  pinned at the top.

### 5.5 Keyboard *(from C)*

| Keys | Action |
|---|---|
| ⌘K / Ctrl-K, `/` | Palette (or focus the page filter when one exists) |
| `g` then `h` `l` `m` `p` `w` `b` `e` `o` `c` `d` | Home, Live, Media, Podcasts, Workspaces, Briefings, Email, Operations, Costs, Data |
| `j` / `k`, Enter | Move row focus in the page's primary table, open or inspect |
| `[` | Collapse or expand the rail |
| Esc | Close palette, sheet, inspector or modal |
| `?` | Shortcut sheet |

Shortcuts ignore keystrokes in inputs, textareas, selects and contenteditable.
`g` hints show as `kbd` on rail items only on hover or focus, to keep the rail
quiet.

## 6. Components

All components live in `omni-web-kit/src/components/` and are the only
primitives; per-page re-implementations are deleted. The shell pieces (Rail,
TopBar, TabBar, Palette, Connection, Clock, ShortcutSheet) live in
`crates/omni-web/src/shell/`. StatusSentence is `PageHead` with `sentence`, and
Stage is `Panel` with `stage`. Tones are one Rust enum
(`Tone::{Neutral, Live, Ok, Warn, Fault, Info, Signal}`) mapped to a class
modifier, replacing every `format!("prefix-{}")` tone class.

| Component | Variants | States and rules |
|---|---|---|
| **Button** | `primary` (Signal fill, `--signal-ink` text; one per view), `secondary` (`--raised`, `--line-strong` border), `ghost`, `danger` (fault outline, fills on hover); sizes `md`/`sm`; `icon` (square, needs `aria-label`) | hover (lighter tier or `--signal-hi`), pressed (translateY 1 px), focus-visible ring, `busy` (spinner replaces icon, label width kept, `aria-busy`), `disabled` (`--text-off` text, dashed border, **always with a reason** in `title` and, on phone, a helper line). |
| **ConfirmButton** | wraps any Button | Two-step: first press arms it ("Confirm run", warn wash and outline, 3 s countdown bar along the bottom edge); second press acts; Escape, blur or timeout disarms. Irreversible deletes arm to fault "Delete permanently". Replaces every `window.confirm()`. |
| **RunButton** | inline `sm` (visible on row hover or focus, always visible on phone) and inspector `primary` | ConfirmButton semantics; on success optimistic `running`, button `busy`; HTTP 409 shows the toast "<name> is already running" and does not flip `running` (existing `LiveData::run_task` contract). |
| **Segmented** | range (`30D 90D All`), mode, filters with inline mono counts | Track `--surface` with `--line` border; active segment `--raised` with `--e-2`; `aria-pressed`; zero-count options stay visible but `--text-3`. Replaces `.range-buttons`, `.mode-toggle`, single-select chip groups and the old `StatusFilterChips` (removed). |
| **Chip** | neutral, signal | Multi-select filters and tags; pressed = `--signal-wash` fill plus `--signal-line` border, `aria-pressed`. |
| **Tag** | tone variants, mono 12 px, `--r-xs` | Static labels: tier, kind, policy, trigger (`schedule`, `manual`). |
| **Status** | `ok` (dot only), `running` (spinner arc plus word), `warn` (diamond plus word), `fault` (square plus word), `idle` (ring plus word), `stale` (diamond plus "stale") | Replaces `StatusDot`, `status-chip-*`, `mail-outcome-*`, `mcp-status-*`, `workspace-status`, `claude-tag-*`. Always has a text alternative. |
| **LiveTag** | `LIVE`, `LIVE · 4h 12m` | Pulsing dot; `--live-wash` fill; not rendered when offline. |
| **Readout** | `xl`, `l`, `m` with key, sub-line, optional Delta and Sparkline | Value-tick flash on change (section 9); skeleton bar while loading; `stale` (figure `--text-2`, sub-line "as of 12:10" in warn). |
| **Delta** | up (`--ok`, `↑`), down (`--text-2`, `↓`; a dip is not an error), flat, `surge` (Signal word "Surging" when the backend's surge or anomaly gate fires) | Mono `read-s`. The UI never computes its own surge. |
| **Avatar** *(from B)* | monogram on a deterministic muted two-stop gradient from the id hash, or an image when the data has one; sizes 24/32/40/64 | `live` adds a 2 px `--canvas` gap plus 2 px `--live` ring; `offline` is desaturated (`filter: grayscale(.85) brightness(.7)`), restored on hover. |
| **Panel** | plain; with header (h3 title, mono meta, trailing link) | `--surface`, `--line`, `--r-lg`, `--e-1`. No hover of its own. |
| **Stage** *(from B)* | the one hero panel for something live | Panel plus a static `radial-gradient(120% 140% at 0% 0%, var(--live-wash), transparent 60%)` background and a `--live-line` top edge. Only when live, at most one per page. No blur. |
| **Row** | link row, button row, grid row | hover `--hover`; focus ring inset; selected (`--signal-wash` fill plus 2 px Signal inset on the left). Clickable rows are a single `<a>` or `<button>`. |
| **Table** | header row of `label` text, `rowgroup` group headers (h3 plus mono count plus meta), sticky header | Numeric columns right-aligned mono; the title column ellipsizes (`max-width:0; width:100%`). Per-column `hide-below-desk` / `hide-below-wide`. Phone: two-line rows (name plus key figure; mono meta line). Empty row "No tasks match · Clear filter". Loading: five skeleton rows at real height. |
| **RunStrip** | last N runs as 4×14 px cells (N = 12 desktop, 8 phone) | success `--bar-hi`; failure `--fault`, 18 px tall; running Signal, breathing; skipped `--warn` diamond-topped; missing dashed outline. Each cell has a tooltip with time, duration and summary. |
| **TimeLane** *(from B)* | horizontal lane over the last 10 minutes plus 2 ahead, with a now-line | Runs are 2×12 px ticks colored as RunStrip; scheduled runs hollow; used only for realtime tasks. |
| **Meter** | 96×6 track; fill = value/max; optional reference tick | Used for session length, peak vs record, budget, score rows. Fill `--bar-hi`; Signal only for the top item. |
| **Sparkline** | 1.5 px line, 12% area wash, end dot with 2 px surface ring, optional dashed reference line | Line `--text-2` by default, Signal for the hero; live hero area uses `--live-wash`. |
| **Disclosure** | one `<details>` style for every expand | 14 px chevron rotates 90° over `--d-2`; content does not animate height. Label says what and how many: "Show 34 older streams", "Transcript excerpt · 75 s". Replaces "Details >", "HISTORY v", "Show More 29" and custom toggles. |
| **ShowMore** | ghost button at the end of a list | "Show 24 more" with the remaining count. |
| **Inspector** | right drawer 440 px (720 px with `wide`) (desk), docked pane 400 px (wide, Operations and Email only), bottom sheet 88dvh (phone) | Header: Status, title, primary action, close (`.inspector-close`, focused first by `use_modal`). Escape and scrim close; focus restored; deep-linkable by URL hash. |
| **Modal** | only for the LogViewer full-screen mode and Data JSON on phone | Same `use_modal` contract. |
| **LogWell** | mono 12/18 grid of time, level, logger, message on `--well` | Level colors: debug `--text-3`, info `--text-2`, warn `--warn`, error `--fault`. Live: a Signal `▍ streaming` cursor while the SSE tail is open. Empty: "No log lines were captured for this run." Dropped-lines notice in warn. Download button kept. Phone: time and level on one line, message below. |
| **Palette** | section 5.4 | |
| **Toast** | one global region in the shell, bottom-right (above the tab bar on phone) | Status shape plus one line; 4 s; `role="status"`, fault uses `role="alert"` and stays until dismissed. Replaces per-page `use_toast` instances (the API can stay, backed by a shell context). |
| **Tooltip** | chart tooltip (`.chart-tooltip`) and hint tooltip | `--overlay`, `--e-2`, value line `read-s` in `--text`, context line `small` in `--text-3`. |
| **Skeleton** | text line, readout, poster, row | `--raised` block with a 1.4 s shimmer; geometry matches the final layout. |
| **EmptyState** | glyph tile, one sentence, one action | e.g. "Paste an article URL to make your first episode". |
| **ErrorState** | inline (in a panel) and page | What failed, why if known, Retry, and a link to Operations or setup. Never shows a raw API string as the headline (raw detail goes in a Disclosure). Defines the missing `.error-banner` look. |
| **StatusSentence** | the hero of overview pages | `display` type; deviating words take their hue and link to the cause. Loading is a skeleton line, never "Checking in…". |
| **Poster** | 2:3 (movies), 1:1 (podcasts, episodes) | Real TMDB or artwork with `loading="lazy"`, explicit aspect ratio, `--r-lg`, 1 px inner `--line`. Fallback: title set in `h2` on a muted gradient derived from the title hash. Hover: lift 2 px and a `--line-strong` outline (no blurred color bloom; it costs too much on phones). |
| **PlatformIcon** | YouTube, Twitch, Kick, DGG | 16 px, fills from section 2.5. |
| **Kbd** | mono 11 px on `--raised` with `--line-strong` border | |

Usage rules:

- One primary button per view. Run, Approve and Watch are the usual primaries.
- Every disabled control says why.
- Navigation is a link (`<a>` via `Link`), actions are buttons. A row that
  navigates is a link row, never a button with `navigate`.
- Destructive actions are ConfirmButtons, never `window.confirm()`.
- Do not nest panels. Inside a panel use rows, hairlines and group headers.

## 7. Data visualization

- **Form first.** A single number is a Readout, not a chart. Change over time is a
  bar chart (daily buckets) or line (continuous samples). Proportion of a total
  is a Meter or a table with inline bars, not a pie.
- **Marks.** Bars ≤ 22 px wide with 4 px rounded data ends anchored to the
  baseline and a 2 px surface gap; lines 2 px; markers ≥ 8 px with a 2 px surface
  ring. Gridlines `--grid` solid hairlines, 3 to 5 ticks; axis text `--axis`
  `read-s`. No dual axes.
- **Single-series bars** (streamer daily peaks) use
  `--bar`. Only the bar that matters is highlighted: today (in progress) hatched
  with Signal at 45°; the all-time record outlined in Signal with a `★ 3,558`
  label; the hovered bar `--bar-hi`. Days with no data are a 2 px dot at the
  baseline, labelled "No stream" (or "No data") in the legend, so gaps never read
  as missing data. Optional dashed reference line for typical peak, labelled at
  the right end.
- **Multi-series** (costs by feature): series palette in fixed entity
  order; legend always present; ≤ 4 series are also direct-labelled; legend items
  toggle series. A "Table" toggle shows the same data as a table.
- **Lines** (pets weight, session curve): Signal line for the subject, neutral
  area wash; brush (Pets) as a thin overview track under the chart, shown only for
  All with more than 60 points.
- **Hover and touch.** Charts use pointer events (mouse, pen, touch). Bars: a
  full-height hover band plus tooltip. Lines: crosshair plus tooltip. On touch, a
  tap pins the tooltip until the next tap outside the chart; a drag scrubs.
  Keyboard: the chart is focusable, arrow keys move the selection, the tooltip
  follows.
- **Text never wears series colors.** Values and labels use text tokens; a colored
  swatch carries identity.
- **Hooks:** keep `.chart-container` (sized parent) and rename the tooltip class to
  `.chart-tooltip` together with `charts.rs`. Update SVG `var(--…)` references in
  `charts.rs`, `nav_bar.rs` and `platform_icon.rs` to the new tokens in the same
  change.
- **Session sparkline data.** The snapshot has no viewer series. The SPA keeps a
  per-tab ring buffer of `viewerCount` per live streamer, filled from each
  snapshot (SSE or poll), capped at 4 h / 720 points. On a cold load the line
  starts at "now". An optional later backend field (`recentViewers`, downsampled)
  may seed it; the layout does not depend on it.

## 8. Live data and status

The data layer is unchanged: one `/api/events` SSE per tab, 10 s
`/api/snapshot` poll while the stream is down, 5 s reconnect, keyed per-id memos,
`use_visible_poll`, per-run log SSE, `workspace-updated`.

### 8.1 Connection

Rail footer (desktop) and a dot in the phone top bar, mapped 1:1 onto
`ConnectionState`:

| State | Look |
|---|---|
| Live | Signal dot that pops once per snapshot (600 ms) *(from C)*, "Live · 2s" (seconds since last snapshot, `use_now(1000)`) |
| Polling | warn diamond, "Polling · every 10 s" |
| Connecting / Reconnecting | warn diamond pulsing, "Reconnecting…" |
| Error | fault square, "Offline · retry" |

Any panel whose data is older than twice its refresh interval shows
`updated 3m ago` in warn in its header.

### 8.2 Live viewers

- Updated in place through the existing keyed per-id memos; only the text node
  changes. Rows and cards are never re-created, so hover and focus survive.
- The changed figure gets the value tick (section 9).
- The rail On air rows, Home, `/live` and the streamer page read the same memo.
- Order on Home and `/live`: relevance (`intelligence.relevanceScore`), then
  viewers, with a segmented `Relevance | Viewers` sort. Reordering only happens
  when the order actually changes.

### 8.3 Tasks

- Running: spinner Status, Signal task name, breathing last RunStrip cell, busy
  RunButton, an elapsed timer in mono.
- Countdowns tick every second: under a minute `0:06`, longer `4h 46m`, `1d 20h`.
  At zero they show "due" until the snapshot confirms the run.
- **Stale** *(from C, new client-side rule)*: a scheduled task whose last run is
  older than three times its period shows the `stale` Status and joins the
  attention list. Computed only from the schedule and `lastRun` in the snapshot.
  Realtime tasks use the same rule.

### 8.4 Freshness while refetching

Previous data stays visible at 60% opacity with a 2 px Signal progress line at
the top of the panel; nothing reflows. This is the Costs behavior generalised.

### 8.5 Attention

The Home status sentence and the "Needs you" panel aggregate: pending workspace
actions, failing tasks, stale tasks, email failures, MCP errors in 24 h and an
unconfigured or offline Claude host link when sessions are expected. Each item
links to the filtered view (`/operations?filter=attention`, `/emails?outcome=failed`).

## 9. Motion

| What | How | Reduced motion |
|---|---|---|
| Value tick | digits flash `--signal` and fade to `--text` over 900 ms; no count-up, no layout shift (tabular figures) | instant color change, held 900 ms |
| Live dot | 2 s ring ping (scale and opacity) | static dot |
| Connection pop | dot scales 1 → 1.6 → 1 over 600 ms per snapshot | none |
| Running | 0.9 s spinner arc; breathing RunStrip cell | static half-filled glyph |
| Disclosure | chevron rotation `--d-2` | instant |
| Drawer, sheet, palette | translate plus opacity `--d-3` | instant |
| Toast | rise 8 px plus fade `--d-3` | fade only |
| Deep-link target | 2 s Signal outline pulse (`deep-link-target`) | static outline for 2 s |
| Armed confirm | 3 s bar drains along the button bottom | static bar |
| Press | translateY 1 px `--d-1` | none |

Only `transform` and `opacity` animate (plus color for the tick). The reduced
motion block is the only place `!important` is allowed.

## 10. Loading, empty and error

- **Loading:** skeletons shaped like the final content. Never "Loading…" text on
  its own. Previously loaded data stays visible while refetching (8.4).
- **Empty:** EmptyState with one sentence and one action. Examples: Pods "Paste
  an article URL to make your first episode"; Media "No picks yet · Run picks";
  Emails filtered to nothing "No failed emails in the last 500 · Clear filter";
  Briefings "No briefings yet"; MCP "No calls in this window".
- **Error:** ErrorState inline in the panel that failed; the rest of the page
  keeps working. Page-level ErrorState only when the page's primary request fails.
  GET retries (502/503/504) stay in the API client; the UI shows a skeleton until
  they give up.
- **Configuration states** are designed, not errors: Claude host unconfigured
  ("The Claude Code host isn't linked. Set `OMNI_DEVICE_LINK_TOKEN` on the server
  and run `omni-link` on the host."), host offline (last seen, what it blocks),
  Reminders disabled ("Disabled: no `ICLOUD_REMINDERS_*` config").

## 11. Accessibility

- Contrast: all readable text ≥ 4.5:1 on every surface it sits on (`--text-3` is
  the floor at 4.8:1 on `--overlay`); `--text-off` is decoration only. Status
  hues on canvas: live 6.1, ok 11.4, warn 11.0, fault 7.4, info 9.5, Signal 15.4.
- Status is never color alone: each has a shape and a word (2.4).
- Focus: `:focus-visible` 2 px Signal outline with 2 px offset everywhere; inset
  on rows. Never removed.
- Kept as today: skip link, focus to `#main-content` on navigation,
  `aria-current`, `aria-pressed`, `aria-expanded`/`aria-controls`, `use_modal`
  focus trap, Escape, scroll lock and focus restore, `role="status"`/`"alert"`.
- Targets: ≥ 44 px on phone (buttons 44, small buttons 36 with 8 px spacing,
  rows 56, tabs 44 plus label).
- Charts: focusable, arrow-key navigable, legend plus Table toggle for
  multi-series.
- `prefers-reduced-motion`: section 9. `forced-colors`: status shapes and
  outlines remain; hatching uses `CanvasText`.
- Live regions: viewer counts are **not** live regions (too chatty); the
  connection state and toasts are.

## 12. Route plan

Shared shape for every page: top bar crumbs and actions; then the page hero; then
content. "Readout band" is a row of 3 to 5 `read-l` Readouts in one Panel with
hairline dividers (2 columns on phone).

### `/` Home

1. **Status sentence** (display) with a mono date/time label above: "All quiet."
   plus a `body` line "4 channels on air, every task healthy, nothing waiting on
   you." With deviations: "2 things need you." with linked colored phrases.
2. Wide: two columns (8/4).
   - Left, **On air Stage**: header (LiveTag "On air", "4 channels · 41,786
     watching", segmented Relevance/Viewers). The lead channel (highest
     relevance): Avatar 64 live ring, name, platform, tier Tag, uptime, title (h2),
     current chapter (h3 plus summary, left rule), `read-xl` count with Sparkline
     and dashed typical-peak line, four facts (session peak, Δ vs 15-min baseline,
     DGG with Delta, typical peak). Other live channels: rows with Avatar 32,
     name and title, uptime, DGG, Delta and right-aligned `read-m` count.
     Footer Disclosure "9 channels offline · 3 primary" opens desaturated Avatar
     chips (primary first) with last-live time.
   - Right: **Needs you / Nothing needs you** panel (check rows: workspace
     actions, task failures, stale tasks, email failures, each linking) and
     **Up next** (next non-realtime runs with countdowns; realtime tasks collapsed
     to one line "Realtime checks ×4 · 15–30 s").
3. **On deck**: four real posters (2:3) with title, year, one-line why, picked
   age; "All picks" link. Phone: snap rail.
4. Three panels (4/4/4): **Research** (active subjects with pending actions),
   **Inbox** (processed, filtered, failed readouts plus latest parcel events),
   **Spend** (30-day `read-l`, daily mini bars, largest feature).
5. Phone order: sentence, lead Stage, other live rows, Needs you, Up next, On
   deck rail, Research, Inbox, Spend. The old System Health row is removed.

### `/live` (new)

- h1 "Live" and a status sentence ("4 of 13 on air").
- **On air** table: Avatar, name plus title, platform, uptime, DGG, Delta, Meter
  (now vs session peak, typical-peak tick), `read-m` viewers. Sort segmented.
- **Offline** table grouped by tier (Primary, Background): Avatar desaturated,
  name, last live (relative), last peak, typical peak. Rows link to the streamer.
- Phone: two-line rows.

### `/streamers/:id` live (e.g. `/streamers/hutch`)

1. Hero: Avatar 64 with live ring, LiveTag with uptime, tier Tag ("Background
   tier" explains muted notifications in its tooltip), name (display), title
   (body, `--text-2`), binding chips (platform, handle, primary flag, DGG embed
   count). Actions: "Intelligence" (secondary) and primary "Watch on <platform>".
2. Readout band as a **Stage**: Watching now (`read-xl`, value tick) with the
   session Sparkline and dashed typical-peak line *(from B)*; Session peak (since
   08:01); DGG with Delta; Typical peak with all-time record and date.
3. Wide two columns (8/4):
   - Left: **Daily peak viewers** chart (section 7; 30D/90D/All, default 90D;
     "51 of 90 days streamed" meta). **Streams** grouped by week (group header:
     label, stream count, hours, week peak); each row: date block (day number plus
     weekday), title, duration Meter plus start time and length, `read-m` peak
     (Signal plus ★ for the record; peaks ≥ 1.3× typical in `--text` weight 600).
     The live session is pinned on top with a LiveTag. First 4 weeks, then
     ShowMore "Show 34 older streams".
   - Right (sticky): **Now** panel: session lane from start to now with chapter
     ticks, current chapter (h3 plus summary), Tags for confidence and relevance,
     "Is this summary right?" Accurate / Off (existing intelligence-feedback POST),
     Disclosure "Transcript excerpt · 75 s". **Baseline** panel *(from C)*: key
     values current (sampled), baseline 5–20 min, change, slope, DGG now/base,
     baseline samples, surge gate (typical peak), suppression, updated time. It
     makes the surge rules visible without computing anything new. **Bindings**
     (per platform viewers, primary). **Records** (all-time peak, longest stream,
     streams tracked).
4. Phone order: hero, readouts, Now, chart (200 px tall), streams (two-line rows),
   Baseline, Bindings, Records.

### `/streamers/:id` offline (e.g. `/streamers/destiny`)

Same layout without the Stage glow: Avatar desaturated, no LiveTag; hero line
"Last live 15h ago · peak 2,436"; readout band shows Last peak, Typical peak,
All-time, Streams in 30 days. Actions: "Intelligence" and primary "Open
channel". The Now panel becomes "Last session" with its final chapter, and the
Baseline panel is hidden.

### `/streamers/:id/intelligence`

- Crumbs `Live / Destiny / Intelligence`. A sticky summary bar under the top bar:
  current topic, confidence Meter, stage health as Status items, budget Meter,
  "updated 12s ago".
- Segmented `Timeline | Diagnostics`.
  - **Timeline**: chapters as group headers; events grouped under them; runs of
    "summary updated" collapse to "Summary updated ×14 · 10:00–10:58" (expandable).
    Filter chips above (default "Topics and anomalies"). Evidence and transcript
    excerpts are Disclosures.
  - **Diagnostics**: health and stage grid as a Table, metrics, budget detail.
- Errors use ErrorState (styles `.error-banner`). The 10 s loop becomes
  `use_visible_poll` (stops while hidden).
- Target length: about three viewports instead of 15,000 px.

### `/media` (and `/recommendations`)

- Top bar actions: Picks Segmented (1 to N) plus RunButton "Run picks"
  (two-step; 409 toast).
- Hero: **On deck** rail of large posters.
- Segmented by status with counts (Pending, Watched, Not for me, Dismissed…; the
  existing statuses). Poster grid: 6 columns wide, 4 desk, 2 phone (phone may
  switch to a list with 64 px thumbnails via a `Grid | List` toggle). At most 24,
  then ShowMore.
- Card: poster, title and year, kind Tag, one-line why, feedback buttons
  (icon plus label). Clicking the card opens the **Inspector** with the full
  detail (why, caveats, links, scores, timeline, note); "Open page" links to
  `/media/:id`.
- Taste Brain: right panel on wide (key-value bars), Disclosure otherwise.
  Recommendation runs: a link "Runs in Operations" to
  `/operations#inspect=Recommendations` (the panel is removed here).
- `?recommendation=<id>`: select the target's status group first, scroll to
  `#recommendation-<id>`, apply `deep-link-target`.

### `/media/:id` (e.g. `/media/582207dc-…`)

- Two columns wide: poster (320 px) left; right: kind and year Tags, title (h1),
  service links as secondary buttons (Plex, TMDB), why (body, 68ch), caveats.
- Scores as Meters (Signal for the top score). Timeline as a vertical rail with
  mono times. Feedback as Segmented plus note textarea `#rec-feedback-note`, save
  button. Phone: poster 60% width centered, sticky feedback bar at the bottom.

### `/podcasts` and `/podcasts/:id` (e.g. `/podcasts/6fcb0401-…`)

Same system as Media with 1:1 artwork, "featuring" line under the title, queued
state as an `info` Status. The detail page adds the feed URL (mono, copy button)
and keeps `#podrec-feedback-note`. Castro links to `/podcasts/<id>` keep working.

### `/feedback/recommendations/:id` and `/feedback/podcasts/:id`

Focused mode: no rail, no tab bar; top bar shows only "Omni" and "See details".
Single column, max 480 px: artwork with title and why, three 56 px full-width
choices, optional note, Save. Saved state: large check, the chosen option, "See
details" link. Works one-handed at 390 px.

### `/pods`

- Hero: the submit field (`#article-url`, 48 px, inline error
  `#article-url-error`) with primary "Make episode".
- **Queue**: rows with Status, source domain, progress lane, elapsed, retry and
  dismiss (ConfirmButton), Logs (inspector LogWell).
- **Episodes**: list rows with 1:1 generated art (gradient from the source
  domain), title, source, duration, date, inline play button that expands an
  inline player row; options menu (retry, delete as ConfirmButton). ShowMore.
- Empty: EmptyState "Paste an article URL to make your first episode".

### `/pods/:id`

- Sticky player bar under the top bar: art, title, `<audio class="pods-detail-audio">`
  via `NodeRef`, download link.
- Two columns wide: left the chapter list (mono time column; buttons seek and
  focus the player, labels "Jump to <title> at <time>"), right the transcript as
  15/24 prose in 68ch, collapsed preview with "Read full transcript"
  (`#episode-transcript`).
- **Processing** section: chunks Table (warnings, re-split Tags), retriever
  attempts (winner Tag), cost breakdown; admin retry and delete as ConfirmButtons
  then `navigate("/pods")`.

### `/workspaces`

h1 "Research", status sentence ("1 subject active · 0 actions waiting").
Workspace rows (not cards): title, description (one line), subject counts,
pending actions badge, schedule (mono), "Start research" compose entry per
workspace.

### `/workspaces/:w` (e.g. `/workspaces/purchase-research`)

Header: title, scope description, counts readouts, next sweep countdown. Subject
Table (status, title, updated, pending actions). Compose form (textarea with the
workspace placeholder, primary Send) docked at the top.

### `/workspaces/:w/:s` (e.g. `/workspaces/marketplace-selling/f241c120-…`)

- Header: crumbs, subject title (h1), status `<select>` styled as Segmented menu,
  updated time.
- Wide: two panes. Left sticky outline (200 px): Actions, Artifacts (each artifact
  by name), Conversation, Sources, Papercuts, with counts. Right: the selected
  section.
- **Actions** pinned first when any are pending: cards with title, payload
  Disclosure, Approve (primary) and Reject (danger ConfirmButton), result line.
- **Artifacts**: one at a time with tabs per artifact key; Markdown at 15/24,
  68ch, revision count.
- **Conversation**: thread with the composer docked at the bottom of the section;
  run progress shows a running Status while the 1 s log poll is active.
- `?section=` selects the section; `target=action-<id>` / `artifact-<key>`
  selects the right tab first, then scrolls and applies `deep-link-target`.
  Element ids `action-<id>`, `artifact-<key>` and anchors `#actions`,
  `#artifacts`, `#conversation`, `#sources` are kept (the selected section's
  anchor always exists in the DOM; other sections render collapsed, not
  removed, so the ids exist).
- Phone: the outline becomes a sticky Segmented (Actions, Artifacts,
  Conversation, Sources) plus a select for the artifact.

### `/briefings`

Day-grouped feed with source Chips. Each briefing is a row (source Tag, title,
time, cost in mono) that expands in place to the message (15/24 prose) and a
Logs button (inspector LogWell). ShowMore.

### `/emails`

- Status sentence ("28 screened today, none failed.") and readout band
  (processed, filtered, failed, cost).
- Toolbar: pipeline Segmented, outcome Segmented with counts, filter field.
- Mail Table: outcome Status, subject, from, pipeline Tag, time, feedback Tag;
  grouped by day. Rows open the Inspector (wide: docked) with logs, reprocess,
  feedback, delete delivery (ConfirmButton) and "Add rule from this sender".
- Sender rules move to a `Activity | Rules` Segmented: built-in lists as
  read-only Disclosures, user rules as rows with delete, add-rule form inline on
  one line (input plus two selects plus Add).

### `/data`

Full width. Wide/desk: entity list left column (240 px) with counts; phone:
`<select id="data-entity-select">`. Storage summary readouts, search field,
Table with sticky header, mono keys, dense rows; row opens the Inspector with the
JSON in a LogWell-styled block; download button; delete is a ConfirmButton.
ShowMore.

### `/costs`

Range Segmented (7D, 30D, 90D, All; default 30D) in the top bar. Readout band
(range total `read-xl`, average per day, highest day, events). Stacked bar chart
by feature with legend toggles and Table toggle. Breakdown as two Tables (feature,
model) with inline proportion Meters. Recent events Table. Stale-while-loading
per 8.4.

### `/operations`

1. Status sentence ("17 tasks, all healthy.") and readout band: healthy x/y,
   running (names), failing, stale, next run with live countdown.
2. Toolbar: filter field (`/`), Segmented All / Attention / Running with counts
   (`?filter=` in the URL).
3. **Task Table** grouped by cadence: Realtime (sub-minute), Frequent (minutes to
   hours), Scheduled (daily and weekly). Columns: Status, name (h3) with mono id
   and last summary on the second line (summaries starting with "skipped:" in
   warn), cadence (human plus dim raw cron), last run (relative plus duration),
   history (TimeLane for Realtime, RunStrip of 12 for the rest), next (countdown),
   RunButton.
4. Row click, Enter or `j/k` selects and opens the **Inspector** (docked pane on
   wide, drawer on desk, sheet on phone; `#inspect=<Task>`): Status, schedule,
   next 3 runs, primary RunButton, last run (time, trigger, duration, run id),
   run history (12 rows, each with summary and a Logs button), and the latest
   run's **LogWell** with the live SSE tail while running.
5. **Opening a run's logs**: the Logs button swaps the inspector's LogWell to that
   run; "Expand" opens the full-screen LogViewer modal (filters by level,
   download).
6. **Run log** below the table: recent runs collapsed by repeats
   ("LiveCheckTask ×12 · all succeeded · 615 ms avg"), with a task filter and an
   `All | Errors` Segmented.
7. Phone: two-line rows (name plus next; RunStrip plus last), inspector as sheet.
   The HISTORY accordions and `task-history-<name>` ids become inspector
   sections (keep the id on the inspector history list).

### `/pets`

One Panel per pet: name, latest weight `read-l` with Delta, visits count,
Segmented Weight / Visits and range Segmented (7D, 30D, 90D, All), line chart
(Signal line, neutral area, brush per section 7), CSV export as a ghost button.
Two pets side by side on wide.

### `/mcp-activity`

Readout band (calls 24h, errors, approvals, p50 latency). Tools Table (tool,
calls, errors, average ms, policy Tag, RunStrip of recent outcomes). Toolbar:
tool select plus status Segmented. Calls Table (dense) whose rows open the
Inspector with args and result JSON. "Load older" stays as ShowMore.
`use_visible_poll` 10 s unchanged; freshness note in the panel header.

### `/claude`

- Host link hero: Status (online, offline, disabled, unconfigured), explanation
  and setup steps; facts (host, last seen, pending jobs) as readouts. No raw API
  strings in the headline.
- Sessions Table (project, state Status, last turn, turns); "Include stopped"
  Chip. Rows open the transcript in the Inspector (chat bubbles, tool chips as
  Disclosures, Markdown).
- Action timeline grouped by session on a vertical rail; repeated results
  collapse; "Other calls" as a dense Table with ShowMore. The UI may say "Mac".

### `/reminders` (server page)

Stays a full page load with the same CSP and element ids (`insecure`,
`controls`, `status`, `diagnostic`, `code-form`, `reminders-code`,
`submit-code`, `start`, `verify`, `error`). `reminders.html` gets the token
values copied into its inline `<style>` (allowed by `style-src 'unsafe-inline'`),
drops the light-mode branch, and uses the system fallback stacks (same-origin
fonts are optional). Layout: a 52 px top bar with the Omni mark and "← Omni"
(plain link to `/`), then one centered 560 px Panel: Status line, explanation,
buttons in the shared Button styles, code input (mono, 16 px), diagnostic in a
LogWell-styled block. The insecure notice is a warn ErrorState. `reminders.js`
changes only if a hook id moves (it should not).

### 404 (e.g. `/nope-404`)

Page EmptyState: "Nothing at" plus the path in mono, the palette search field
focused, a Home button, and the five main destinations as link rows. Styles the
existing `not-found-page`.

## 13. Implementation contract

### 13.1 CSS structure

`style/index.css` is replaced by ordered layers declared in `tokens.css`:
`@layer reset, tokens, base, components, layout, pages, utilities;`
Files, each linked from `index.html` as its own Trunk-hashed stylesheet:
`tokens.css` (layer order, `@font-face`, every token), `base.css` (reset, type,
reduced motion), `components.css` (all kit components in one file, grouped by
component), `layout.css` (shell, rail, top bar, tab bar, grids),
`pages/ops.css` and `pages/domain.css` (page exceptions for `omni-web` and
`omni-web-pages`). About 2,300 lines. No `!important` outside reduced motion;
no raw colors outside tokens.

### 13.2 Rust class and token coupling (change together)

- `use_modal` focus target `.log-modal-close` becomes `.inspector-close`
  (inspector, modal) and `.palette-close` (palette sheet); `.mobile-more-close`
  and `#mobile-more-menu` are removed with the More sheet (the palette sheet
  takes `#nav-palette`).
- `deep-link-target` is kept.
- `.chart-container` kept; `.custom-tooltip` becomes `.chart-tooltip` with
  `.chart-tooltip-row/-label/-swatch`.
- `conn-<state>` becomes the Status tone on the connection component.
- All `format!("prefix-{}")` tone classes become `Tone` enum modifiers
  (`status ok`, `tag warn`, …).
- SVG `var(--…)` renames: `--bg-card` → `--surface`, `--text-muted` → `--text-3`,
  `--border` → `--line`, `--accent-soft` → `--signal-wash`, `--accent` →
  `--signal`, `--live` unchanged.
- `index.html`: `theme-color` `#0a0b0d`; favicon becomes a Signal ring on
  canvas (inline SVG data URI); font preloads.

### 13.3 Must not change

Routes and their tests (plus the new `/live` tests), `/recommendations` alias,
trailing-slash and percent-decoding rules, `page_title()` and dynamic streamer
titles; query and hash deep links (`?recommendation=`, `?section=&target=`);
ids `article-url`, `article-url-error`, `data-entity-select`,
`rec-feedback-note`, `podrec-feedback-note`, `episode-transcript`,
`workspace-summary`, `task-history-<name>`, `recommendation-<id>`,
`action-<id>`, `artifact-<key>`, section anchors; SSE plus poll fallback and
reconnect; keyed in-place live updates; the 409 toast semantics; PressPods
refetch triggers; recommendation reload on run change; workspace run polling and
`workspace-updated`; audio chapter seeking; `/reminders` full load and CSP; no
inline scripts in the SPA (boot via hashed `js/boot.js`).

### 13.4 Performance budget

- No new crates, no JS dependencies, no CDN. Fonts are two variable woff2 files
  (about 140 KB total), preloaded and served from `assets/fonts/` under stable
  names (section 3.1).
- CSS about 105 KB unminified across the six files (Trunk does not minify it);
  keep it from growing and prefer kit components over new page rules.
- No `backdrop-filter` except the top bar; no `filter: blur` anywhere; the Stage
  glow is a static gradient.
- Posters `loading="lazy"` with explicit aspect ratios (no layout shift).
- Long lists cap and page with ShowMore; no virtualization.
- Sparkline buffers in memory only, ≤ 720 points per live streamer.
- Animations only on `transform`, `opacity` and color.
