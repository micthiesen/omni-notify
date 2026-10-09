# omni-notify

Edit `AGENTS.md` and `.agents/skills/` directly. These are native Codex files;
no generated copies or compatibility links are needed.

Personal automation service for livestream monitoring, email processing, AI
briefings, recommendations, PressPods, and durable Omni workspaces. It is a Rust
workspace: one `omni-notify` binary serves the API, MCP endpoint and a Leptos
frontend, and runs every scheduled task. See `docs/architecture.md`.

Update this file when work establishes or changes a durable project convention.

## Work and delivery

Use `cargo` from the repository root. The toolchain comes from
`rust-toolchain.toml` (rustup installs it on first use, plus
`rustup target add wasm32-unknown-unknown` for the frontend). After code
changes, run:

```bash
cargo xtask gate                         # hygiene, fmt --check, clippy -D warnings
                                         # (native and wasm32), deps-check, tests
cargo deny --locked check                # advisories, licenses, bans, sources
(cd crates/omni-web && trunk build --release)   # when the frontend changed
```

`cargo xtask gate` is the local equivalent of CI's lint-and-test job, except
`cargo deny` (install with `cargo install --locked cargo-deny@0.20.2`, the CI
pin). trunk, wasm-bindgen and wasm-opt versions are pinned in
`deploy/web-tools/install.sh` and `crates/omni-web/Trunk.toml`; wasm-bindgen
must equal the workspace `wasm-bindgen` pin. `cargo fmt --all` fixes formatting.

Build directories grow without bound. The gate starts with
`cargo xtask hygiene`, which measures every Cargo profile directory under
`target/` and, above `OMNI_TARGET_LIMIT_GIB` (default 40), sweeps the oldest
artifacts with `cargo sweep` (`brew install cargo-sweep`) down to half the
limit. Run one heavy build session at a time. Concurrent agents in the same
checkout each set their own `CARGO_TARGET_DIR=target/agents/<name>` (and a
separate one for wasm or trunk work) so they do not block on Cargo's lock or
invalidate each other's artifacts; hygiene covers those directories too.

Completed, reviewed work is authorized to ship directly to `main`: preserve
unrelated changes, commit the scoped result, pull with rebase if the remote moved,
and push. A push deploys production automatically. Do not ask for another
confirmation already supplied by this rule or the current request.

Production runs as the `omni-notify` container on `boris` (`10.10.1.100`) from
`/home/michael/compose`. Boris's deploy timer also keeps the
`executor-events-adapter` sidecar (the `omni-events-adapter` crate) on
`:latest`; see `deploy/executor-events/`. Persistent data is under
`/home/michael/compose/volumes/omni-notify`. The LAN UI is
`http://omni.boris/`. Inspect failures with `docker logs omni-notify` over SSH.

Prefer a committed boot migration for reproducible data changes. For one-off
database surgery, stop the container and make a WAL-safe backup first. Direct
destructive production data changes require authorization specific to that
change; reuse authorization already given in the conversation.

## Rust conventions

- **Dependencies:** every external dependency is declared once in the root
  `[workspace.dependencies]` with an exact `=x.y.z` pin; crates use
  `workspace = true`. `Cargo.lock` is committed and every build and test uses
  `--locked`. `deny.toml` allows crates.io sources only and a fixed license list.
  The toolchain is pinned in `rust-toolchain.toml` and matches the Dockerfile
  `rust:` images and `rust-version`.
- **Lints:** every crate has `[lints] workspace = true`. `unsafe_code` is
  forbidden. `unwrap`, `expect`, `dbg!` and `print!`/`eprint!` fail the gate
  outside tests (`clippy.toml` allows unwrap and expect in tests; `xtask` opts out
  of the print lints). Dropped futures are denied.
- **Errors:** expected failures are `Result` values with a `thiserror` enum per
  domain module (`ImapError`, `CaldavError`, `ParcelError`, ...). Wrap foreign
  errors at the leaf; give retry-relevant errors an `is_transient()`-style
  method. `anyhow` is for binaries, `xtask` and tests only. Panics are bugs; they
  are caught and logged only at task and request boundaries. HTTP handlers return
  `ApiError`, which renders `{"error": "..."}`.
- **Concurrency:** tokio, structured. Spawn through
  `omni_core::spawn::spawn_tracked` on the app `TaskTracker`; nothing is
  fire-and-forget. Work that must not be split by a dropped request or shutdown
  (durable reservation, external call, record) runs inside `must_complete`. Fan
  out with `JoinSet` or `buffer_unordered(n)` with explicit limits. Use
  `tokio::sync::Mutex`/`Semaphore` for async critical sections. All "now" comes
  from `ctx.clock` and all sleeping from `tokio::time`, so
  `#[tokio::test(start_paused = true)]` controls time. Schedules are
  `omni-tasks` cron tasks, not raw intervals.
- **Logging:** `tracing` only. Each module declares `const LOG: &str = "Name";`
  and logs with `target: LOG`; the name is what the UI log viewer shows. Events
  inside a task run's span are captured into that run's bounded log by
  `omni_tasks::RunLogLayer`, including across `.instrument`ed async work. ERROR
  events reach Pushover through the gated, throttled `AlertLayer`. Never log
  secrets, Claude session prompt text, or email bodies at info.
- **Ports:** a subsystem crate depends only on foundation crates. Calls into
  another subsystem go through the port traits in `omni_runtime::ports`, wired
  once in `crates/omni-notify/src/wiring.rs`; handle an unset port. The only
  direct subsystem edges are `omni-ios-controls -> omni-live`,
  `omni-parcel -> omni-email` and `omni-calendar -> omni-email`;
  `cargo xtask deps-check` enforces this.
- **Side effects:** every mutating external adapter honors `SideEffectMode`. In
  `Record` mode (tests, shadow runs, `--preview`) it records instead of sending.
  New adapters must do the same.
- **Persisted data:** the docstore format is shared with existing production
  rows. Payloads use the node-cbor-compatible codec in `omni-store`; entity
  structs keep unknown fields in `#[serde(flatten)] extra: Extra`; use
  `Option` with `skip_serializing_if` for absent fields and a plain `Option` for
  stored nulls. Values derived with JS semantics and then persisted or hashed
  (UTF-16 lengths, `JSON.stringify`, `Number#toString`, `localeCompare`, date
  parsing) go through `omni_core::js` only. Each subsystem lists its
  `EntityDescriptor`s; boot runs `migrate_all` over them, and
  `omni-notify compat-audit` checks a production copy against them.
- **Validation:** decode untrusted external, persisted, environment, AI and
  protocol values into typed serde structs before use. MCP tool inputs and model
  structured outputs are also checked against their JSON schemas (`jsonschema`).
- **Idempotency:** make notifications, queue operations, external writes and
  retries idempotent before reporting success. Reserve durably before the side
  effect, record after, and never retry an uncertain outcome automatically.

## Crate map

- `omni-core`: clock, ids, `js` compat helpers, digests, email model and handler
  trait, `spawn_tracked`/`must_complete`.
- `omni-config`: typed environment config, validation, redaction, defaults.
- `omni-store`: SQLite docstore actor, node-cbor codec, entities, `migrate_all`,
  relational tables.
- `omni-tasks`: cron scheduler, task registry, durable run history, run log
  capture, catch-up, event buses.
- `omni-http`: reqwest client with the project user agent, bounded bodies,
  SSRF-guarded public client, `SideEffectMode`.
- `omni-alerts`: Pushover client, alert throttle, ERROR-log alert layer and gates.
- `omni-mailer`: outgoing SMTP and MIME with the fixed sender identity.
- `omni-ai`: model registry, provider clients, tool loop, cost accounting.
- `omni-server-kit`: axum helpers (body caps, errors, origin guard, SSE, SPA).
- `omni-mcp-kit`: MCP tool interface and golden tool metadata.
- `omni-runtime`: `AppContext`, `Subsystem`, boot steps, services, ports.
- `omni-api`: wire DTOs shared by backend and frontend (serde only).
- `omni-testkit`: temp stores, test clock, recorders, fake models, no-network
  HTTP, golden helpers (dev-dependency only).
- `omni-imap`: iCloud IMAP transport, cursors, archive actions, drafts, Sent
  copies, compose, email MCP reads.
- `omni-email`: email dispatch, retry queue, watchdog, activity, triage, sender
  rules, feedback.
- `omni-parcel` and `omni-calendar`: Parcel and calendar email handlers
  (CalDAV writes in `omni-calendar`).
- `omni-live`: `channels.json`, aggregate streamer state, notifications, viewer
  metrics and records, DGG discovery, `LiveCheckTask`.
- `omni-ios-controls`: signed iOS control routes, live slots, APNs pushes.
- `omni-live-intel`: livestream audio capture, local speech and speaker models,
  viewer anomalies, Destiny presence, summaries, alerts.
- `omni-presspods`: article retrieval, narration, speech, audio chain, storage,
  RSS.
- `omni-podcasts`: podcast recommendations, taste reflection, Castro bridge and
  Inbox cleanup.
- `omni-media`: Plex, TMDB, Radarr/Sonarr, media recommendations, taste
  reflection.
- `omni-arr`: `ArrRecovery` and `ObserverRepair`.
- `omni-reminders`: server iCloud Reminders and the shared protected-access
  (PCS) workflow; callers own authenticated requests, encrypted cookie
  persistence and per-account serialization.
- `omni-workspaces`: durable conversational workspaces.
- `omni-briefings`: scheduled briefing agents and history.
- `omni-mcp`: authenticated `/mcp` endpoint, MCP Events outbox, MCP activity,
  system/events/Claude session tools, policy inventory.
- `omni-device-link`: the Claude Code host's `omni-link` long-poll relay.
- `omni-personal`: pets, LAN printer, Hister, and Codex/Claude reset alerts
  (`reset_alerts`).
- `omni-notify`: the binary: boot, wiring, ops routes, dashboard SSE, data
  manager, `doctor`, `healthcheck`, `compat-audit`, `--preview`.
- `omni-web`, `omni-web-kit`, `omni-web-pages`: the Leptos CSR frontend (app
  shell and ops pages, shared kit, domain pages), built by trunk.
- `omni-events-adapter`: the `executor-events-adapter` sidecar binary.
- `xtask`: gate, hygiene, deps-check, golden capture and synthesis, MCP policy,
  compat and API diff tooling.

The frontend is Leptos 0.8 client-side rendering built by trunk into
`crates/omni-web/dist` and served from `OMNI_WEB_DIST`. It talks only to the
REST API in `omni-api`. Live data arrives over one `/api/events` SSE stream per
tab and falls back to polling `/api/snapshot` every 10 s while the stream is down.

## Failure-sensitive invariants

### Livestreams

YouTube discoveries resolve video ownership through YouTube metadata and durable
identity links. Never infer YouTube ownership from display names. A video linked
to its configured YouTube account enriches DGG presence without adding a second
viewer-count source. Failed identity revalidation preserves the last verified link.

A streamer is an aggregate identity over platform bindings. Notify only on the
aggregate offline-to-live and live-to-offline edges. The first live binding is
the sticky primary for the session; a primary switch is silent. Viewer counts
sum currently live bindings. `channels.json` is the source of truth and invalid
configuration must fail boot rather than silently unmute a streamer.

`tier: "background"` mutes live, offline, and title notifications, records only
all-time viewer highs, and polls every third tick. It cannot be combined with an
explicit `liveNotifications` value. Title changes use an eager debounce: the
first change sends immediately, later changes within ten minutes collapse to the
last title, and offline or primary-switch transitions clear pending state. A
viewer peak becomes a record only after count falls 5 percent below it; flush a
pending peak when the stream goes offline. Primary-tier streamers notify 7, 30
and 90-day window records as well as all-time records; the window maximum is
taken from history before the current observation, and a record already
confirmed today does not repeat (`omni-live/src/metrics/`).

Viewer surges compare against a 5-20-minute-old baseline that must be flat, not
still climbing after go-live, and a platform surge must also reach the median
peak of recent full sessions. Sparse baselines after a restart or primary switch
suppress rather than alert.

Missing, empty or corrupt livestream speech or speaker model files disable
livestream intelligence at boot (`Livestream intelligence disabled: ...`) and
leave live checks running; they never crash boot.

### Email

- Dispatch is no-drop: events received during processing schedule another pass,
  and transport cursors commit only after dispatch. Message-ID is the stable
  identity across folder moves.
- iCloud IMAP uses per-folder UID cursors without CONDSTORE/QRESYNC. Re-read
  capabilities after authentication. Keep the seven-day INTERNALDATE guard that
  cursor-skips bulk imports, and on UIDVALIDITY change replay only from the
  last-dispatch watermark before reseating the cursor.
- Filter in this order: user block, user allow, static blacklist, static
  auto-pass, then shared LLM triage. Explicit user rules override built-ins.
- Activity outcomes must reflect per-item success. A fully rejected submission
  cannot be recorded as processed.
- Parcel extraction separates order numbers from tracking numbers, validates
  ranked carrier candidates against the live list, and uses durable dedup.
- Calendar output is sanitized before persistence. Cancellations require an
  explicit event reference; receipts and bills never imply cancellation.
- CalDAV discovery follows RFC 6764 from principal to home set to a VEVENT
  collection. Never hardcode an iCloud `pXX` shard. Cross-calendar moves can
  return 403 and require delete plus recreate.
- Network and 5xx failures enter the durable retry queue. Re-fetch by email id;
  handler dedup makes replay safe.

### Notifications and runs

Throttle each notification path at exactly one layer. Preserve distinct incident
keys for distinct users, URLs, subjects, or tracking numbers. Logs emitted during
a tracked task run remain attributable through async work and are bounded before
persistence.

`CodexResets` polls Reset Beacon's public alert feed and history every minute.
Keep predictions, announcements, observed rollouts and reported landings distinct.
Never infer a landing from an elapsed deadline or a banked grant from an ordinary
reset. Completed history can precede the alert feed; deduplicate both by source
post as well as event. Keep pushes concise with a source button and preserve
durable delivery reservations. See `docs/codex-resets.md`.

`ClaudeResets` polls Reset Radar's structured catalog for confirmed historical
Claude Code counter-reset reports. Tracker confirmation is not account
verification. Preserve banked-reset wording without inferring redemption or
expiry. The providers share bounded reads, scheduling, and delivery code in
`omni-personal`'s `reset_alerts` module, with separate durable namespaces and
unchanged Codex keys. See `docs/claude-resets.md`.

### PressPods

Retrievers run independently and the metadata model selects the best usable
article. Speech is chunked and verified before the finished episode is exposed.
Every sample-rate conversion in
`crates/omni-presspods/src/speech/audio_chain.rs` must retain `RESAMPLE_HQ`; the
Higgs denoise path retains `FIZZ_SHELF`. See `docs/presspods-audio.md` before
changing the audio chain.

Higgs denoise uses the `arnndn` filter with
`assets/press-pods/denoise.rnnn` (`DENOISE_MODEL_ASSET`); the model must remain
in the Docker image and runtime FFmpeg must include that filter.
`omni-notify doctor --image` checks both at image build.

### Castro

Inbox preview cleanup (`CastroInboxCleanup`) runs every six hours
(`0 */6 * * *`). Keep failed runs visible, but gate Pushover on at least three
consecutive failures spanning twelve hours (allowing five minutes of schedule
jitter), using durable run history (`omni-podcasts/src/castro/alert_gate.rs`).
Success resets the incident; isolated HTTP 500 and socket failures must not
notify.

### Arr recovery

`ArrRecovery` requires an unchanged explicit import failure across observations
spanning 15 minutes; never bypass that gate for manual runs. Luna may interpret
names but cannot override exact target mappings or unsafe import rejections.
Reserve mutations durably and verify imports/deletions before reporting success.
Search only missing monitored targets, with durable retry limits. See
`docs/arr-recovery.md` for NZBGet health checks and shared temporary-path mounts.

### Observer issue repair

`ObserverRepair` uses Luna to interpret reports and deterministic Arr tools to
replace scoped media or search missing files. Preserve exact TMDB/TVDB identity,
file-to-import-to-grab provenance, and the report scope ceiling. Reserve mutations
durably; never repeat uncertain deletions/searches automatically. Resolve only
after search acceptance and a verified explanatory comment, then Pushover notify.
Unsupported issues stay open and notify once. See `docs/observer-repair.md`.

### Workspaces and MCP

Server iCloud Reminders is independent of Mac EventKit and IMAP/CalDAV. It stays
disabled without its complete `ICLOUD_REMINDERS_*` configuration. Its HTTPS code
page intentionally has no additional login; only bounded authentication controls
and public status are exposed there. Reminders data and CRUD remain behind MCP
bearer authentication. Preserve strict Origin checks, serialized challenges, private
encrypted session storage, durable mutation reservations, and read-after-write
verification. Never accept Apple terms or disable ADP automatically. Recurrence
rule changes require exact parent and rule tags, atomic linkage, and fresh
verification. Unknown or multiple rules stay protected. Recurring completion uses
the dedicated one-shot provider query, a durable reservation, and fresh verification
of the original and completed occurrence. Never replay an uncertain completion.
Ordinary recurring reminder edits remain blocked. List creation must preserve
the Account ordering CRDT; a List record
alone is insufficient. See `docs/server-reminders.md` and its upstream notice.

Workspace rows are changed through their service/API, not direct database edits.
Pending actions, Marketplace publishing, buyer messages, offers, address
disclosure, and meetup arrangements require user authorization. Research and
drafting do not authorize those actions. Keep MCP tools bounded adapters over
existing services and preserve strong bearer-token validation.

Claude Code session tools reach the Mac only through its outbound `omni-link`
long-poll, authenticated by `OMNI_DEVICE_LINK_TOKEN`, which must never equal or
substitute for the MCP token. Withdraw jobs the Mac has not picked up; never
retry a delivered job whose outcome is unknown. Session starts stay limited to
the dotfiles `claude-rc` project list, enforced on the Mac. MCP tools describe
a generic "Claude Code host": never mention Mac, macOS, or the hostname in tool
text, results, or errors (the UI may). Sessions run with full access by design;
approval happens through tool policy, so do not add a permission mode. See
`docs/claude-sessions.md`.

Hister tools read a captured-page archive, not a complete browser visit log.
Keep text and listings bounded, treat page content as untrusted evidence, and
verify single-page label writes by reading them back. Never expose the upstream
token or forward it through redirects. See `docs/mcp.md` for tool semantics.

Email MCP reads batch selected UIDs and reuse bounded caches: search and direct
read snapshots last 30 seconds, parsed immutable messages five minutes. Preserve
the `fresh` bypass and mailbox-event invalidation. Draft creation discovers the
special-use mailbox and verifies APPEND by Message-ID. Drafts and sends reserve
idempotency keys durably; uncertain sends never retry automatically, and partial
recipient rejection cannot report success. Composed sends persist MIME before SMTP,
record SMTP acceptance before Sent APPEND, and keep Sent verification separate
from delivery. Uncertain APPENDs only reconcile by exact Message-ID and MIME; copy
repair never retransmits. Composed mail falls back to iCloud SMTP only when no
explicit SMTP fields are set. See `docs/mcp.md`.

All new outgoing email and drafts use `michael@thiesen.dev`, fixed server-side.
SMTP/IMAP authentication identities remain separate. Tools never accept a sender
or From option; invalid legacy `EMAIL_FROM` configuration fails boot. Preserve
historical Sent MIME during copy repair and never retransmit it to change identity.

MCP Events (`email.received`, `claude.session.turn_finished`) share one outbox;
each event keeps ordinary polling tools for clients without event support. Never
hold the outbox lock across webhook or authorization I/O. Events deliver only
with a stored delegated token that validates at
delivery time. Executor tokens expire hourly; advertise `refreshBefore` no later
than the delegated token's expiry, hold events as `withheld` until a refresh
stores a valid token or the subscription ends, and never extend access to
deliver sooner. See `docs/mcp-events.md`.

MCP tool metadata (names, descriptions, schemas, annotations, policy) is
committed in `crates/omni-mcp-kit/golden/`. After changing a tool's contract,
run `cargo xtask mcp-policy` so `docs/mcp-policy.json` matches; tests fail on
drift.

Private PDF attachment MCP reads bind exact Message-ID to actual MIME part IDs,
revalidate identity, and cap source and decoded bytes. Preserve read-only/PEEK
semantics, declared MIME validation, safe download names, and private binary
responses without public storage or server-selected filesystem destinations.
Outgoing send and draft attachments are only `attachmentReference` values
(`{messageId, attachmentId, sha256}`) from `email_attachment_get`, re-read through
that path, never caller bytes. The required SHA-256 must match before any APPEND
or SMTP; keep the PDF-only scope tied to that reviewed-read path. Resolve them before reserving the
idempotency key, settle known keys from their receipt first, and keep attachment
bytes in the persisted MIME so Sent copy repair never re-reads a source. Receipts
drop wire MIME once SMTP completes and the private copy once it is verified.

## Code and tests

- rustfmt with `max_width = 100` (`rustfmt.toml`); Clippy for correctness.
- Prefer strong types, enums for discriminated states, small modules, and
  explicit returns.
- Do not leave debug logs, commented-out code, or unnecessary abstractions.
- Tests are spec-style acceptance tests: integration tests in `crates/*/tests/`
  named after the behavior they cover, plus `#[cfg(test)]` unit tests for pure
  decisions. Test pure decisions separately from network adapters. Async tests
  use `#[tokio::test(start_paused = true)]` and the test clock; assert typed
  failures with `matches!`.
- Use `omni-testkit` fakes (`TestStore`, `TestApp`, `FakeModels`, recorded
  Pushover and mail, `no_network`, `mock_http` over wiremock). Tests never reach
  the network.
- Tests against a production copy are `#[ignore]` and read `OMNI_PROD_COPY`; run
  them with `cargo test -p <crate> --test prod_copy -- --ignored`. Live
  integration smoke tests are also `#[ignore]` and run manually with dotenvx.
- Committed golden fixtures are synthetic only. Capture raw responses with
  `cargo xtask capture-golden` into the gitignored `.local/golden-capture/`, then
  derive committed fixtures with `cargo xtask golden-synthesize`;
  `crates/omni-api/tests/golden_privacy.rs` rejects personal data. Never commit a
  production database or capture. The repository is public.

For ad-hoc scripts that need configured credentials, write a scratch example and
delete it afterward:

```bash
# crates/<crate>/examples/omni-<subject>.rs, removed when done
dotenvx run -- cargo run -p <crate> --example omni-<subject>
```

## Secret files

`.env` contains unrecoverable secrets.

- Never print or read secret values into the conversation. List key names with
  `cut -d= -f1 .env`; test presence with an opaque `grep -qE` check.
- Never rewrite, delete, or edit existing `.env` lines. If an existing value must
  change, ask Michael to edit it.
- New placeholders may be appended. `.env.example` is version controlled and can
  be edited normally.
- Consume secrets through `dotenvx`; do not export or interpolate them into shell
  commands.

For web requests made with curl or an equivalent raw client, set the user agent
to `OpenAI File Downloader, XaiImageApiFetch/1.0`.
Exception approved by Michael: the server Reminders Apple client retains ioBroker's
endpoint-specific Apple-compatible User-Agent and Referer headers. This exception
does not apply to other HTTP clients.
