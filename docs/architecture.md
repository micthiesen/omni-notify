# Architecture

omni-notify is one Rust binary (`omni-notify`) that runs scheduled tasks, an axum
HTTP server with the JSON API, MCP endpoint and Leptos frontend, and long-lived
background services over one SQLite document store. A second binary,
`executor-events-adapter`, is the MCP Events sidecar deployed beside Executor.

## Crate layout

The workspace (`Cargo.toml`, `members = ["crates/*"]`) has four layers. Crate
names are `omni-*`; library names are `omni_*`.

- **Foundation:** `omni-core`, `omni-config`, `omni-store`, `omni-tasks`,
  `omni-http`, `omni-alerts`, `omni-mailer`, `omni-ai`, `omni-server-kit`,
  `omni-mcp-kit`, `omni-runtime`, `omni-api`. They know nothing about features.
- **Subsystems:** `omni-imap`, `omni-email`, `omni-parcel`, `omni-calendar`,
  `omni-live`, `omni-ios-controls`, `omni-live-intel`, `omni-presspods`,
  `omni-podcasts`, `omni-media`, `omni-arr`, `omni-reminders`, `omni-workspaces`,
  `omni-briefings`, `omni-mcp`, `omni-device-link`, `omni-personal`. Each exposes
  a constructor that returns an `omni_runtime::Subsystem`.
- **App:** `omni-notify` builds the `AppContext`, constructs every subsystem,
  wires ports, and owns boot, the ops routes, dashboard SSE, the data manager and
  the CLI.
- **Outside the backend layering:** the frontend crates `omni-web`,
  `omni-web-kit` and `omni-web-pages`; the sidecar `omni-events-adapter`; the
  dev-only `omni-testkit`; and `xtask`.

`cargo xtask deps-check` (part of the gate and CI) enforces the direction:

```
omni-core <- {omni-config, omni-store} <- omni-tasks <- omni-runtime <- subsystems <- omni-notify
omni-core <- omni-http <- {omni-alerts, omni-ai, omni-mailer} <- omni-runtime
omni-api (serde only) <- omni-server-kit, subsystems, frontend crates
```

Subsystem crates must not depend on each other, except for three allowlisted
library edges: `omni-ios-controls -> omni-live`, `omni-parcel -> omni-email` and
`omni-calendar -> omni-email`. Frontend crates may use only `omni-api` and each
other. Nothing depends on `omni-notify`.

## Composition and ports

`omni_runtime::AppContext` carries the shared handles: config, store, clock,
HTTP clients (`HttpClient`, SSRF-guarded `PublicHttpClient`), Pushover, the
optional mailer, model client and cost recorder, task registry, event bus,
shutdown token, `TaskTracker`, ports, `SideEffectMode` and resolved paths.

A `Subsystem` contributes its axum router (routes at absolute paths), tasks, MCP
tools, entity descriptors, data-manager rows, boot steps, background services,
email handlers and alert gates. `omni-notify` merges the routers, registers the
tasks and tools, and starts the services.

Cross-subsystem calls go through the port traits in
`crates/omni-runtime/src/ports.rs`: `EmailReader`, `ArchiveEcho`,
`EmailRetryHandlers`, `CalendarWriter`, `LiveDirectory`, `LiveIntelligence`,
`OnDeckSource`, `BriefingsReader`, `ClaudeSessionNotifier` and `ClaudeHost`. Each
port is set once during wiring (`crates/omni-notify/src/wiring.rs`); a consumer
must handle an unset port, because the providing subsystem may be disabled by
configuration. Port payloads that belong to another subsystem's `omni-api` DTO
cross as `serde_json::Value` and are decoded into the same DTO on the other side.

`SideEffectMode::Record` makes every mutating adapter (Pushover, SMTP, IMAP
writes, CalDAV, Arr/Castro/Plex mutations, APNs, webhooks, printer, Parcel)
record instead of sending, while reads stay live. Tests and shadow runs use it;
select it with `omni-notify --side-effects=record` or `OMNI_SIDE_EFFECTS=record`.

## Boot order

`crates/omni-notify/src/boot.rs`:

1. Load config and log it redacted.
2. Open the store.
3. Construct subsystems (they do not read the docstore in constructors).
4. `migrate_all` over every subsystem's entity descriptors.
5. Import historical costs.
6. `Migrate` and `Services` boot steps.
7. `registry.initialize` marks interrupted runs.
8. `Reconcile` boot steps (interrupted MCP calls, calendar hash reconcile).
9. Start the HTTP server.
10. `AfterServer` boot steps.
11. Register tasks, start background services (email features restart with
    30 s to 300 s backoff, MCP delivery worker, Reminders health check), start the
    scheduler, then run catch-up recovery as tracked work.

`--server-only` stops after step 10. Shutdown cancels the token, stops accepting
requests, closes MCP and SSE streams, and waits up to 30 s for tracked work.

Livestream intelligence is optional at boot: if its speech or speaker model
files are missing, empty or corrupt (`omni-live-intel/src/model_check.rs`), boot
logs `Livestream intelligence disabled: ...`, installs no `LiveIntelligence`
port and continues.

## Data compatibility

The production database is the SQLite file at `DB_NAME`
(`/data/docstore.db` in the container), created by the earlier TypeScript service.
Its format is a contract:

- One connection on a dedicated thread, WAL, `synchronous=NORMAL`,
  `busy_timeout=5000`, writes in `BEGIN IMMEDIATE`.
- The `blobs` table keys are `$<entity>#` plus parts joined by `#`
  (`s<utf16 len>:<value>`, `n<number>`, `b1`/`b0`). Reads hide expired rows.
- Payloads are CBOR encoded exactly as node-cbor 10 would encode the same JS
  value (`crates/omni-store/src/cbor/`): shortest integers, f32 when lossless,
  tag 1 dates, tag 258 sets, definite maps in field order. The committed vectors
  in `crates/omni-store/tests/golden/cbor.json` freeze that behavior.
- Every entity struct carries `#[serde(flatten)] extra: Extra`, so a
  read-modify-write never drops fields it does not know.
- Values derived with JS semantics and then persisted or hashed (UTF-16 lengths,
  `JSON.stringify`, `Number#toString`, `localeCompare`, date parsing) go through
  `omni_core::js` only.
- Entity migrations run at boot through `migrate_all` from each subsystem's
  `EntityDescriptor` list. Reproducible data changes are boot migrations, not
  manual edits.

`omni-notify compat-audit --db <copy> [--rewrite-to <new.db>]` checks a copy of
production: per-entity row counts, CBOR and typed decode failures, primary-key
recomputation, unknown or legacy rows, and the typed round trip
`decode -> typed -> encode -> decode`. The source file is copied to a temporary
directory first and never opened in place. `cargo xtask compat-audit --db <copy>`
runs the row-level subset that needs only the foundation crates. Take the copy
with `sqlite3 docstore.db ".backup /tmp/copy.db"` on boris, keep it mode 600
outside the repository, and point `OMNI_PROD_COPY` at it to run the ignored
`prod_copy` tests.

## Golden fixtures

External contracts are frozen by committed fixtures: HTTP JSON responses
(`crates/omni-api/tests/golden/http/`), MCP `tools/list`, handshake and policy
(`crates/omni-mcp-kit/golden/`), the PressPods RSS feed and ffmpeg arguments,
CBOR vectors, MIME parsing, config parsing, cron parity and JS-compat cases.

Committed fixtures are synthetic. `cargo xtask capture-golden --base URL`
captures read-only GETs and the MCP handshake from a running server into the
gitignored `.local/golden-capture/`; `cargo xtask golden-synthesize` derives the
committed HTTP fixtures from that capture, replacing emails, IDs, URLs, IPs and
free text consistently and failing if any replaced value leaks.
`crates/omni-api/tests/golden_privacy.rs` scans every committed fixture for
personal markers. `cargo xtask golden-check` verifies the MCP policy and that
committed fixtures are unchanged.

MCP tool metadata in `crates/omni-mcp-kit/golden/` is the source of truth for
each tool's public contract. After changing it, run `cargo xtask mcp-policy` to
render `docs/mcp-policy.json` and its golden copy.

## Frontend

`omni-web` is a Leptos 0.8 client-side app built by trunk
(`crates/omni-web/Trunk.toml`) into `crates/omni-web/dist`. `omni-web-kit` holds
the API client, live data, hooks, components, SVG charts and markdown;
`omni-web-pages` holds the domain pages. The REST contracts in `omni-api` are the
only API; there is no SSR.

Live data uses one `EventSource("/api/events")` per tab. The server sends a ping
immediately, then the client's initial snapshot, then debounced broadcasts.
While the stream is down the client polls `/api/snapshot` every 10 s and shows a
reconnecting badge. Run logs stream from `/api/task-runs/:id/logs/stream`.

The server serves the built app from `OMNI_WEB_DIST` (`/app/web` in the image):
hashed `/assets/*` are immutable and Brotli-precompressed, `index.html` is
`no-cache`, and unknown `/api/*` GETs return JSON 404. trunk injects no inline
script, so the `/reminders` CSP needs only `'wasm-unsafe-eval'`.

## Deployment pipeline

`Dockerfile` stages, all pinned and SHA-256 verified:

- download stages for cargo-chef, the sherpa-onnx static library, trunk /
  wasm-bindgen / wasm-opt (`deploy/web-tools/install.sh`), yt-dlp and the three
  livestream models;
- `chef -> planner -> server`: cargo-chef cooks dependencies, then one
  `cargo build --release --locked` builds `omni-notify`, `omni-voice-enroll` and
  `omni-intel-doctor`. The server stage excludes the web crates,
  `omni-events-adapter` and `xtask`, so editing them keeps it cached;
- `web -> web-dist`: `trunk build --release`, then Brotli and gzip copies;
- `runtime` on `node:24.19.0-slim` (Node stays for `yt-dlp --js-runtimes node`),
  with ffmpeg, CUPS/brlaser, ghostscript, poppler and tini. Layers run from least
  to most frequently changed so a code change pushes only the last layers.
  `RUN omni-notify doctor --image` fails the build when ffmpeg filters, the
  denoise model, printer files, sherpa models or yt-dlp are missing.

The Rust stages rely on cargo-chef layers, not cache mounts, because the GitHub
Actions cache exports layers but not mounts.

CI (`.github/workflows/ci.yml`): `lint-and-test` runs fmt, native and wasm32
clippy with `-D warnings`, `cargo deny --locked check`, `cargo xtask deps-check`
and `cargo test --workspace --locked`, using `Swatinem/rust-cache`. `image` builds
the Dockerfile in parallel with `type=gha,mode=max` layer caching. `publish`
pushes `ghcr.io/micthiesen/omni-notify:latest` and `:<sha>` after both pass, from
the fully cached build. The `Executor Events Adapter` workflow builds and
publishes the sidecar image from `deploy/executor-events/Dockerfile`.

On boris, `omni-notify-deploy.timer` pulls `:latest` every minute, recreates the
container on change, waits for its health check (`omni-notify healthcheck`) and
sends a Pushover notice. The same pass keeps `executor-events-adapter` current.
