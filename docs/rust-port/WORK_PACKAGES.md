# omni-notify Rust port: work packages

Companion to `ARCHITECTURE.md` (APIs, conventions, compatibility contract). Every
TypeScript file under `src/`, `frontend/src/` and `packages/executor-events-adapter/`
is owned by exactly one package (coverage checklist at the end). Ownership is
file-disjoint on the Rust side too: a package writes only inside its own crates, its
own `crates/omni-api/src/<module>.rs` files (ARCHITECTURE.md 3.12), and appends
lines to `[workspace.dependencies]`.

## How to use this document

- **Order**: WP00 first (its day-1 stub commit unblocks everyone; its golden capture
  unblocks contract tests). WP01-WP13 and WP15-WP16 then run in parallel. WP14 integrates
  continuously (it compiles against whatever subsystem crates exist, using `Subsystem::default()`
  stubs) and owns the cutover. WP17 is required and can run in parallel with the subsystem packages.
- **Definition of done for a package**: crates compile with workspace lints; every listed
  spec is ported (or each dropped case has a reason in the test file header); golden
  fixtures for its routes/tools/feeds pass; its `EntityDescriptor`s decode 100 percent of
  the corresponding rows of the production copy in `compat-audit`; no network in tests;
  no `TODO` left in production paths.
- **Sizes** are TS lines including specs, to balance staffing.
- "Spec files to port" = the `[spec]` entries of the package's checklist section; each
  becomes `crates/<crate>/tests/<stem>.rs` (or an inline `#[cfg(test)]` module for pure
  functions), same case names in snake_case.
- **Side effects to stub** are what tests must replace with wiremock, recorders or fakes
  (`omni-testkit`). They are also the adapters that must honor `SideEffectMode::Record`.
- "Foundation APIs" names the `ARCHITECTURE.md` section-3 surface the package relies on.

## Package overview

| WP | Name | Crates | TS files | Size (TS lines) |
|---|---|---|---|---|
| WP00 | Foundation | omni-core, omni-config, omni-store, omni-tasks, omni-http, omni-alerts, omni-mailer, omni-ai, omni-server-kit, omni-mcp-kit, omni-runtime, omni-api (skeleton + common/tasks/runs/costs), omni-testkit, xtask | 51 | ~7.0k + mitools semantics |
| WP01 | IMAP transport, archive, compose | omni-imap | 32 | ~10.0k |
| WP02 | Email pipeline core + Parcel | omni-email, omni-parcel | 45 | ~6.4k |
| WP03 | Calendar events + CalDAV | omni-calendar | 25 | ~5.0k |
| WP04 | Livestreams + iOS controls | omni-live, omni-ios-controls | 54 | ~9.6k |
| WP05 | Livestream intelligence | omni-live-intel | 25 | ~5.6k |
| WP06 | PressPods | omni-presspods | 61 | ~7.8k |
| WP07 | Podcast recommendations + Castro | omni-podcasts | 58 | ~9.0k |
| WP08 | Media recommendations | omni-media | 45 | ~8.0k |
| WP09 | Arr recovery + Observer repair | omni-arr | 28 | ~6.9k |
| WP10 | Server iCloud Reminders | omni-reminders | 23 | ~10.0k |
| WP11 | Workspaces + Briefings | omni-workspaces, omni-briefings | 27 | ~5.1k |
| WP12 | MCP server, Events, Device link | omni-mcp, omni-device-link | 35 | ~9.0k |
| WP13 | Personal services (pets, printer, Hister, reset alerts) | omni-personal | 34 | ~5.5k |
| WP14 | App wiring, ops routes, data manager, cutover | omni-notify | 5 | ~3.2k + integration |
| WP15 | Frontend shell, kit, ops pages | omni-web, omni-web-kit | 45 | ~9.6k |
| WP16 | Frontend domain pages | omni-web-pages | 16 | ~6.2k |
| WP17 | Executor events adapter | omni-events-adapter | 6 | ~1.4k |

## Cross-package dependency rules

- Subsystem crates depend on foundation crates only, plus three allowlisted edges:
  `omni-parcel -> omni-email` (same WP), `omni-ios-controls -> omni-live` (same WP), and
  `omni-calendar -> omni-email` (activity/retry library functions; WP02 publishes their
  signatures on day 1: `omni_email::activity::record(&Store, NewActivity)`,
  `omni_email::retry::enqueue(&Store, pipeline, email_id, reason)`).
- Cross-subsystem calls use `omni_runtime::ports` (ARCHITECTURE.md 3.11). Port
  implementers and consumers:

| Port | Implemented by | Consumed by |
|---|---|---|
| `EmailReader` (fetch_by_id, search, health) | WP01 | WP02 (retry, reprocess, MCP email tools), WP11 (source excerpts) |
| `ArchiveEcho` | WP01 | WP12 (`recordEmail` suppression) |
| `EmailHandler` (omni-core) | WP02 parcel, WP03 calendar, WP11 workspaces, WP12 events | WP02 dispatcher (order wired by WP14: McpEvents, ParcelTracker, CalendarEvents, Workspaces) |
| `EmailRetryHandlers` | WP14 | WP02 |
| `CalendarWriter` | WP03 | WP11 (action approval, UID `workspace-<actionId>@omni-notify`) |
| `LiveDirectory` | WP04 | WP12 (`livestreams_list`, `livestream_get`), WP14 (snapshot), WP05 |
| `LiveIntelligence` | WP05 | WP04 (task hooks, routes), WP12 (`livestream_get`) |
| `OnDeckSource` | WP08 | WP14 (snapshot) |
| `BriefingsReader` | WP11 | WP12 (`briefings_list`) |
| `AlertGate` (omni-alerts) | WP07 (Castro) | WP00 alert layer |

## Route map (server.ts sections and their implementing package)

`src/server.ts` and `src/index.ts` are owned by WP14 for coverage, but the route bodies
are re-implemented by the package that owns the domain, reading these TS line ranges:

| TS lines (server.ts) | Routes | Implementing WP / crate |
|---|---|---|
| 631-671 | `/api/*` same-origin guard | WP00 `omni-server-kit` |
| 655-671 | `registerIOSControlRoutes`, `registerRemindersRoutes` mounts | WP04 / WP10 |
| 672-703 | `/reminders` page headers | WP14 (uses kit CSP helper) |
| 704-719, 849-975, 1049-1062, 1091-1105, 1135-1269, 1924 | tasks, snapshot, `/api/events` SSE, `POST /api/tasks/:name/run`, task-runs, run logs + SSE, health | WP14 `omni-notify::ops` |
| 720-778, 309-397 | streamers, trigger-channels, metrics, sessions | WP04 |
| 779-848 | intelligence-details, intelligence-feedback | WP05 (own router) |
| 976-1048 | data manager | WP14 |
| 1106-1134 | `/api/costs` | WP14 (calls `omni_ai::costs::summarize`) |
| 1063-1090, 1270-1314, 205-265 | media recommendations + run | WP08 |
| 279-308, 1315-1392 | podcast recommendations + run | WP07 |
| 505-576, 1393-1640 | email activity, rules, feedback, parcel delivery delete | WP02 |
| 1641-1668 | briefings | WP11 |
| 188-204, 1669-1923 | workspaces, actions, papercuts | WP11 |
| 1926-1987 | pets, CSV export | WP13 |
| 1984 | `registerPressPodsRoutes` | WP06 |
| 598-630, 1104 | `/mcp`, `/device-link/*`, MCP/Claude activity routes | WP12 |
| 1988-2031 | cache headers, static SPA, shutdown | WP14 + WP00 `spa_service` |

## Task map (all registered in WP14 via `Subsystem::tasks`)

| Task | Schedule (6-field unless noted) | Flags | WP |
|---|---|---|---|
| LiveCheckTask | `*/20 * * * * *` | jitter 3 s, startup | WP04 |
| PetTracker | `0 */10 * * * *` | startup | WP13 |
| PressPods | `0 */5 * * * *` | startup | WP06 |
| ArrRecovery | `0 */5 * * * *` | startup | WP09 |
| ObserverRepair | `0 */15 * * * *` | startup | WP09 |
| CodexResets, ClaudeResets | `0 * * * * *` | startup | WP13 |
| CastroInboxCleanup | `0 */6 * * *` (5-field) | jitter 5 min | WP07 |
| PodcastRecs | `PODCAST_RECS_SCHEDULE` (`0 0 11 * * 1,3,5`) | jitter 5 min, manual input | WP07 |
| PodcastTasteReflection | `0 0 5 * * 0` | jitter 5 min | WP07 |
| Recommendations | `RECS_SCHEDULE` (`0 0 17 * * 1,3,5`) | manual input | WP08 |
| TasteReflection | `0 0 4 * * 0` | | WP08 |
| PurchaseResearch, MarketplaceSelling | `WORKSPACE_SCHEDULE` (`0 0 9 * * 0`) | manual input | WP11 |
| WorkspaceNotifications | `*/5 * * * *` (5-field) | | WP11 |
| Briefings (one per `BRIEFINGS_PATH/*.md`) | front matter | | WP11 |
| EmailRetry | `0 */15 * * * *` | | WP02 |
| EmailWatchdog | `0 0 */6 * * *` | | WP02 |
| EmailArchive | `*/30 * * * * *` | | WP01 |
| McpEventDelivery | `*/30 * * * * *` | | WP12 |
| ClaudeSessionEvents | `*/15 * * * * *` | | WP12 |
| RemindersSession | `*/15 * * * *` (5-field) | | WP10 |
| StoreMaintenance (new, hidden) | `0 17 * * * *` | | WP14 |

---

## WP00 Foundation

**Crates**: omni-core, omni-config, omni-store, omni-tasks, omni-http, omni-alerts,
omni-mailer, omni-ai, omni-server-kit, omni-mcp-kit, omni-runtime, omni-api (skeleton,
`common`, `tasks`, `runs`, `costs`), omni-testkit, xtask. Also root `Cargo.toml`,
`rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `deny.toml`, `.cargo/config.toml`.

**TS files**: `src/effect/**`, `src/utils/**`, `src/types/turndown-plugin-gfm.d.ts`,
`src/ai/**`, `src/alerts/throttle*`, `src/task-runs/**`, `src/costs/**`, `src/emails/**`,
`src/test/mitools.ts`. Reference only (not ported files, but semantics are owned here):
mitools `src/persistence/{sqlite,docstore,entities,table}.ts`, `src/scheduling/Scheduler.ts`,
`src/logging/*`, `src/services/pushover.ts`, `src/boundary`, `src/cli`.

**Milestones** (can be staffed by up to three agents with crate-disjoint ownership):
- F0 (day 1): workspace files; every foundation crate with the exact public signatures of
  ARCHITECTURE.md section 3 and `todo!()`-free stub bodies returning `Err(Unimplemented)`;
  `omni-api/src/lib.rs` declaring all modules with empty files; `omni-runtime::Subsystem`
  with `Default`. Agents for WP01-WP16 start from this commit.
- F1: `omni-core` js-compat + digests (golden vs node: utf16, `JSON.stringify`, number
  formatting, `localeCompare`, `fingerprintEvidence`), `omni-store` (codec, docstore,
  entities, tables, migrate_all, logs_gz), `omni-config`.
- F2: `omni-tasks`, `omni-http`, `omni-alerts`, `omni-mailer`, `omni-server-kit`.
- F3: `omni-ai`, `omni-mcp-kit`, `omni-testkit`, `xtask` (`capture-golden`,
  `golden-cbor`, `mcp-policy`, `golden-check`, `api-diff`, `deps-check`, `node-readback`).
- F4: golden capture run against production (read-only), commit fixtures.

**Spec files to port**: `src/ai/registry.spec.ts`, `src/ai/tools/fetchUrl.spec.ts`, `src/ai/tools/webSearch.spec.ts`, `src/alerts/throttle.spec.ts`, `src/costs/summary.spec.ts`, `src/effect/errors.spec.ts`, `src/effect/http.spec.ts`, `src/effect/interop.spec.ts`, `src/effect/publicHttp.spec.ts`, `src/effect/sse.spec.ts`, `src/emails/client.spec.ts`, `src/emails/mime.spec.ts`, `src/emails/send.spec.ts`, `src/task-runs/catchUp.spec.ts`, `src/task-runs/events.spec.ts`, `src/task-runs/logCapture.spec.ts`, `src/task-runs/persistence.spec.ts`, `src/task-runs/registry.spec.ts`, `src/utils/config.spec.ts`, `src/utils/fingerprint.spec.ts`

Additional acceptance tests (no TS spec exists): CBOR golden vectors from node-cbor
(every number class, Dates incl. sub-ms floats, undefined, Set, Map with non-text keys,
nested objects key order); key encoding with astral characters; expiry filter and
`get_raw_rows_by_prefix`; LIKE escaping and ASCII case-insensitivity; legacy column order
DB; corrupt row behavior per API; `migrate_all` collision skip; scheduler: no overlap,
skip fire during run, jitter bounds, startup trigger mapping, uninterruptible run on
shutdown, `InvalidScheduleError` for never-matching cron; cron parity table (Effect Cron
vs croner) for every schedule in the task map over one year in `America/Vancouver`
including DST transitions.

**Persisted keys owned**: `task-run`, `task-schedule-state`, `task-run-log`, `cost-event`,
`cost-migration` (+ legacy `legacy:briefing:*`, `legacy:press-pods:{llm,tts}:*` ids), and
the `blobs`/`pets` schemas themselves.

**AGENTS.md invariants**: logs during a tracked run stay attributable through async work
and are bounded before persistence (32,768 chars/line, 20,000 lines ring, `dropped`);
throttle each notification path at exactly one layer (the alert layer throttle applies
only to ERROR-log alerts); all HTTP clients use the project UA (Reminders exception lives
in WP10); outgoing mail identity fixed to `michael@thiesen.dev`, invalid `EMAIL_FROM`
fails boot, SMTP fallback to iCloud only when no explicit `SMTP_*` field is set, partial
recipient rejection is never success; secrets never logged (redacted config summary);
deploy uses code-default models (no env override in prod).

**Side effects to stub**: Pushover API, SMTP, OpenAI/Anthropic/Gemini, Tavily, public page
fetches, filesystem log sink.

**Foundation APIs**: n/a (defines them). **Routes**: none (ops routes are WP14).
**Tasks**: none. **MCP tools**: none.

---

## WP01 IMAP transport, archive, compose

**Crate**: omni-imap (modules `transport` actor, `sync`, `map_message`, `attachments`,
`read_cache`, `auto_read`, `drafts`, `sent`, `archive` (imap), `uid_expunge`,
`archive_service`, `archive_store`, `compose`, `mcp_tools`).

**TS files**: `src/email/imap/**`, `src/email/archive/**`,
`src/mcp/tools/email-{compose,archive,attachments}.ts` (+ specs).

**Spec files to port**: `src/email/archive/service.spec.ts`, `src/email/imap/actions.spec.ts`, `src/email/imap/archive.spec.ts`, `src/email/imap/attachments.spec.ts`, `src/email/imap/autoRead.spec.ts`, `src/email/imap/mapMessage.spec.ts`, `src/email/imap/readCache.spec.ts`, `src/email/imap/readCoherence.spec.ts`, `src/email/imap/sent.spec.ts`, `src/email/imap/sync.spec.ts`, `src/email/imap/transport.spec.ts`, `src/email/imap/transportSent.spec.ts`, `src/email/imap/uidExpunge.spec.ts`, `src/mcp/tools/email-archive.spec.ts`, `src/mcp/tools/email-attachments.spec.ts`, `src/mcp/tools/email-compose.spec.ts`

Additional: mailparser golden corpus (`tests/golden/mime/*.eml` → expected `FetchedEmail`
JSON produced by the TS `mapParsedMessage`), including bracketed Message-ID, `partId`
numbering, inline/related attachment set and order.

**Persisted keys owned**: `$imap-folder-cursor#s<n>:<folder>`; raw
`email-archive:action:<sha256hex(idempotencyKey)>` (entity `email-archive-action`),
`email-archive:message:<sha256hex(messageId)>`, `email-archive:history:<sha256hex(messageId)>`;
raw `email-compose:{draft|send}:<sha256hex(idempotencyKey)>` (entities
`email-compose-draft|send`, `StoredAttempt`).

**AGENTS.md invariants**: iCloud IMAP uses per-folder UID cursors, never CONDSTORE/QRESYNC;
capabilities re-read after authentication; seven-day INTERNALDATE guard cursor-skips bulk
imports; on UIDVALIDITY change replay only from the last-dispatch watermark (minus 1 h)
then reseat; Message-ID (with brackets) is identity across folder moves, every HEADER
search confirmed by exact ENVELOPE/parsed match; email MCP caches (search/direct 30 s,
parsed 5 min, `fresh` bypass, invalidation on exists/flags/expunge); draft creation
discovers special-use `\Drafts` and verifies APPEND by Message-ID; drafts and sends
reserve idempotency keys durably, uncertain sends never retry, partial recipient rejection
never succeeds; composed sends persist MIME before SMTP, record SMTP acceptance before Sent
APPEND, Sent verification separate from delivery; uncertain APPENDs reconcile only by exact
Message-ID and semantic MIME; copy repair never retransmits; no From/sender option in tools;
historical Sent MIME preserved; PDF attachment reads bind exact Message-ID to MIME part ids,
revalidate, cap source (20 MiB) and decoded (5 MiB) bytes, PEEK/read-only, declared MIME
validation, safe names, private binary resource responses; only `UID EXPUNGE <uid>`,
never plain EXPUNGE; durable claim before each MOVE/COPY/STORE/EXPUNGE, never repeated.

**Side effects to stub**: IMAP server (in-process fake implementing the `ImapSession`
trait the actor drives; scripted responses incl. COPYUID), SMTP (`Mailer` recorder).

**Foundation APIs**: `Store` raw docs + entities + `write` transactions, `Mailer`
(`send_raw`, `prepare_composed_email`), `Clock`, `spawn_tracked`, `omni_core::email`,
`omni-mcp-kit` (`typed_tool`, custom `ToolOutput` with embedded resource), `TaskRegistry`.

**Implements ports**: `EmailReader`, `ArchiveEcho`. Exposes `ImapTransport::poll()`
returning `EmailPoll { emails, commit }` consumed by WP02's dispatcher via a
`MailSource` trait in `omni_core::email` (WP00 declares it: `poll`, `subscribe_mail_events`).

**Routes**: none. **Tasks**: `EmailArchive` (`*/30 * * * * *`, bounded sweep of 20,
global archive semaphore). **Services**: IMAP connection actor (IDLE on INBOX,
13-min re-IDLE, reconnect backoff 1 s..5 min with 0-3 s jitter, 5-min sweep event).
**MCP tools**: `email_draft_create`, `email_send`, `email_send_status`,
`email_sent_copy_repair`, `email_archive_queue`, `email_archive_status`,
`email_archive_cancel`, `email_archive_restore`, `email_attachment_get`.

**Spike (day 1)**: async-imap surfaces COPYUID/APPENDUID and can issue a lone
`UID EXPUNGE <uid>`; otherwise drive `imap-proto` directly.

---

## WP02 Email pipeline core + Parcel tracker

**Crates**: omni-email (dispatcher, retry + task, watchdog, activity + logs, triage,
sender rules, feedback, link metadata, html-to-text, routes, MCP tools), omni-parcel.

**TS files**: `src/email/*.ts` (top level), `src/parcel-tracker/**`,
`src/mcp/tools/email.ts`, `src/mcp/tools/email-reprocess.ts` (+ spec).

**Spec files to port**: `src/email/activity.spec.ts`, `src/email/activityLogs.spec.ts`, `src/email/dispatcher.spec.ts`, `src/email/feedback.spec.ts`, `src/email/linkMetadata.spec.ts`, `src/email/retry.effect.spec.ts`, `src/email/retry.spec.ts`, `src/email/retryTask.spec.ts`, `src/email/senderRules.spec.ts`, `src/email/triage.spec.ts`, `src/email/watchdogTask.spec.ts`, `src/mcp/tools/email-reprocess.spec.ts`, `src/parcel-tracker/carriers/candidates.spec.ts`, `src/parcel-tracker/carriers/carrierMap.effect.spec.ts`, `src/parcel-tracker/extraction/extractDeliveries.spec.ts`, `src/parcel-tracker/filter/keywords.spec.ts`, `src/parcel-tracker/parcel/parcelApi.spec.ts`, `src/parcel-tracker/persistence.atomic.spec.ts`, `src/parcel-tracker/persistence.spec.ts`, `src/parcel-tracker/pipeline.reliability.spec.ts`

**Persisted keys owned**: `jmap-email-dispatch` (pk `singleton`, name kept), `email-retry`
(`<pipeline>#<emailId>`), `email-activity`, `email-activity-log` (linesGz),
`email-feedback`, `email-sender-rule` (`<scope>:<pattern>`), `parcel-submitted-delivery`.

**AGENTS.md invariants**: no-drop dispatch (bounded(1) coalescing trigger channel; events
during a pass schedule another pass); transport cursors commit only after every handler
succeeded, watermark after commit; filter order user block, user allow, static blacklist,
(AliExpress order-status block), static auto-pass, shared LLM triage (one call per email,
in-flight dedup, failures not cached, 500 cap never evicting in-flight); activity outcomes
reflect per-item success (fully rejected is never `processed`); parcel extraction separates
order numbers from tracking numbers, validates ranked carrier candidates (<=3) against the
live list (24 h cache, stale fallback, refresh mutex), durable dedup with reservation
before POST and near-duplicate check; network and 5xx failures enter the durable retry
queue (30 min x 2^(n-1), 5 attempts), re-fetch by email id, clear only if `enqueueCount`
did not grow; reprocess clears retry only after the handler succeeds; watchdog warns after
72 h without dispatch.

**Side effects to stub**: AI model (triage, parcel extraction), Parcel API
(`/external/add-delivery/`, `supported_carriers.json`), `EmailReader` port, mail source.

**Foundation APIs**: entities, `logs_gz`, `log_capture::capture_scope`, `Ai::generate_object`
+ `ModelRole::{Triage, Extraction}`, `HttpClient::json_bounded`, `TaskRegistry`, `JsonBody`,
`api_error`, `typed_tool`, `ports::{EmailReader, EmailRetryHandlers}`.

**Routes**: `GET /api/email-activity`, `GET /api/email-activity/:activityId/logs`,
`POST /api/email-activity/:activityId/reprocess`, `POST /api/email-activity/:activityId/feedback`,
`GET /api/email-feedback`, `GET|POST /api/email-rules`, `DELETE /api/email-rules/:ruleId`,
`DELETE /api/parcel-tracker/deliveries/:trackingNumber`.
**Tasks**: `EmailRetry`, `EmailWatchdog`. **Services**: `EmailDispatcher` (started by
WP14 with 30 s..300 s exponential retry). **Email handler**: `ParcelTracker`.
**MCP tools**: `email_search`, `email_get`, `email_health`, `email_activity_list`,
`email_activity_get`, `email_reprocess`, `email_rules_list`, `email_rules_upsert`,
`email_rules_delete`, `email_feedback_list`, `email_feedback_set`, `email_retry_list`,
`email_retry_clear`.

---

## WP03 Calendar events + CalDAV

**Crate**: omni-calendar (pipeline, caldav {discovery, http, api, xml, ics}, extraction,
sanitize, filter, persistence, MCP tools, `CalendarWriter` impl).

**TS files**: `src/calendar-events/**`, `src/mcp/tools/calendar.ts`.

**Spec files to port**: `src/calendar-events/caldav/api.spec.ts`, `src/calendar-events/caldav/http.spec.ts`, `src/calendar-events/caldav/ics.spec.ts`, `src/calendar-events/caldav/xml.spec.ts`, `src/calendar-events/extraction/sanitize.spec.ts`, `src/calendar-events/filter/keywords.spec.ts`, `src/calendar-events/persistence.atomic.spec.ts`, `src/calendar-events/persistence.spec.ts`, `src/calendar-events/pipeline.reliability.spec.ts`; plus a new test asserting that `error`,
`no_matches` and final outcome activity rows ARE written (TS defect: missing `yield*` at
`pipeline.ts` lines 289, 329, 401).

**Persisted keys owned**: `calendar-created-event` (pk `eventHash =
"<normalizeTitle>|<startDate>|<startTime|allday>"`, UTF-16 key length matters);
writes `email-activity` / `email-retry` rows through WP02's public functions
(`omni_email::activity::record`, `omni_email::retry::enqueue`), which WP02 exposes as
plain library APIs (omni-calendar may depend on omni-email for these; allowed exception,
add to deps-check allowlist).

**AGENTS.md invariants**: calendar output sanitized before persistence; cancellations
require an explicit `evt_N` reference, receipts and bills never cancel; CalDAV discovery
follows RFC 6764 principal → home set → VEVENT collection, never hardcodes a `pXX` shard,
relative hrefs resolve against the URL that answered, redirects only to trusted iCloud
hosts over HTTPS (max 5); deterministic UID `omni-<sha256(eventHash)[..32]>@omni-notify`
plus `If-None-Match: *` (412 = already exists) makes lost acknowledgements replay-safe;
cross-calendar moves can return 403 and require delete plus recreate (documented
constraint; implement as a guarded path in `update` if a 403 is observed, with a test);
network/5xx enqueue retries; Pushover only after a successful write and record
(`PUSHOVER_CALENDAR_TOKEN`); ICS byte-exact (CRLF, no folding, escape set).

**Side effects to stub**: CalDAV server (wiremock with PROPFIND/PUT/DELETE fixtures from
`xml.spec.ts`), AI model, Pushover, email attachment download (`EmailReader`).

**Foundation APIs**: entities + transactions, `Ai`, `HttpClient::raw` (manual redirects,
streaming caps), `Pushover`, `typed_tool`, `Clock`, `jiff` tz.

**Implements ports**: `CalendarWriter`, `EmailHandler` (`CalendarEvents`).
**Boot step**: `reconcileEventHashes` on first batch (Reconcile phase is acceptable).
**Routes**: none. **Tasks**: none.
**MCP tools**: `calendar_events_list`, `calendar_event_get`, `calendar_status`,
`calendar_event_preview`, `calendar_event_create`, `calendar_event_update`,
`calendar_event_delete`.

---

## WP04 Livestreams + iOS live controls

**Crates**: omni-live (channels config, streamers, task, transitions, title debounce,
notification policy, outage, metrics, sessions, platforms {twitch, kick, youtube}, dgg,
profile links, identity links, display order, trigger channels, routes, `LiveDirectory`),
omni-ios-controls (auth middleware, routes, persistence, APNs, live slots, reconcile).

**TS files**: `src/live-check/**` except `intelligence/`, `src/ios-controls/**`.

**Spec files to port**: `src/ios-controls/apns.spec.ts`, `src/ios-controls/liveSlots.spec.ts`, `src/ios-controls/persistence.spec.ts`, `src/ios-controls/routes.spec.ts`, `src/ios-controls/service.spec.ts`, `src/live-check/channelsConfig.spec.ts`, `src/live-check/dgg.spec.ts`, `src/live-check/displayOrder.spec.ts`, `src/live-check/metrics/persistence.spec.ts`, `src/live-check/metrics/ViewerMetricsService.spec.ts`, `src/live-check/metrics/windows.spec.ts`, `src/live-check/notificationPolicy.spec.ts`, `src/live-check/outage.spec.ts`, `src/live-check/platforms/common.spec.ts`, `src/live-check/platforms/kick.spec.ts`, `src/live-check/platforms/twitch.spec.ts`, `src/live-check/platforms/youtube.spec.ts`, `src/live-check/profileLinks.spec.ts`, `src/live-check/sessions.spec.ts`, `src/live-check/streamers.spec.ts`, `src/live-check/task.spec.ts`, `src/live-check/titleDebounce.spec.ts`, `src/live-check/transitions.spec.ts`, `src/live-check/triggerChannels.spec.ts`

**Persisted keys owned**: `streamer-status` (CBOR tag-1 Dates), `streamer-sessions`,
`streamer-viewer-metrics`, `streamer-platform-viewer-metrics`
(`streamerId, platform, username`), `live-profile-identity-link`,
`ios-control-registration` (`<deviceId>:<controlId>`, `lastDeliveredHash`).

**AGENTS.md invariants**: YouTube discoveries resolve ownership through YouTube metadata
(oEmbed) and durable identity links, never display names; a linked video enriches DGG
presence without a second viewer-count source; failed identity revalidation keeps the last
verified link; notify only on aggregate offline→live and live→offline edges; first live
binding is the sticky primary, primary switch is silent; viewer counts sum live bindings;
`channels.json` is the source of truth and invalid config fails boot; `tier: "background"`
mutes live/offline/title notifications, records only all-time highs, polls every third
tick, cannot be combined with explicit `liveNotifications`; eager title debounce (first
immediate, later within 10 min collapse to last, cleared on offline or primary switch);
viewer peak becomes a record only after a 5 percent fall, pending peak flushed on offline;
went-live persists before notifying, went-offline notifies before recording the session
and writing offline state; iOS: HMAC over `ts\nnonce\nMETHOD\n<raw percent-encoded
path>\nsha256(body)`, ±300 s, single-use nonce recorded after valid signature, 64 KiB body
cap, registration replace is transactional and keeps `lastDeliveredHash` only when slot,
token and environment unchanged; APNs 410/BadDeviceToken deletes only if the token still
matches.

**Side effects to stub**: Twitch GQL, Kick OAuth + API, YouTube pages + oEmbed, profile
pages, DGG websocket (in-process tungstenite server), Pushover (per-streamer tokens), APNs
HTTP/2 (wiremock h2 or trait seam).

**Foundation APIs**: entities, `JsDate`, `Pushover::send_with_token`, `HttpClient`,
`tokio-tungstenite`, `TaskRegistry`, `server-kit` middleware, `api_error`, `omni-config`
channels path, `ports::LiveIntelligence` (optional), `Clock`.

**Implements ports**: `LiveDirectory`.
**Routes**: `GET /api/streamers`, `GET /api/trigger-channels`, `GET /api/streamers/:id/metrics`,
`GET /api/streamers/:id/sessions`, `GET /api/ios-controls/slots/:slot`,
`GET /api/ios-controls/diagnostics`, `PUT /api/ios-controls/registrations`.
**Tasks**: `LiveCheckTask`. **MCP tools**: none (livestream tools are in WP12 via
`LiveDirectory`). **Boot**: load `channels.json` (Services phase, fails boot), drop Kick
bindings without credentials.

---

## WP05 Livestream intelligence

**Crate**: omni-live-intel (audio capture via yt-dlp/ffmpeg subprocesses, local speech via
sherpa-onnx on `spawn_blocking`, anomaly tracker, voice evidence, voice targets, presence
policy, alert policy, classifier, summary text, persistence, service, routes, bins
`omni-voice-enroll`, `omni-intel-doctor`).

**TS files**: `src/live-check/intelligence/**`, `src/types/sherpa-onnx-node.d.ts`,
`src/tools/enroll-destiny-voice.ts`, `src/tools/livestream-intelligence-doctor.ts`.

**Spec files to port**: `src/live-check/intelligence/alertPolicy.spec.ts`, `src/live-check/intelligence/anomaly.spec.ts`, `src/live-check/intelligence/audio.spec.ts`, `src/live-check/intelligence/localSpeech.spec.ts`, `src/live-check/intelligence/persistence.spec.ts`, `src/live-check/intelligence/presencePolicy.spec.ts`, `src/live-check/intelligence/service.spec.ts`, `src/live-check/intelligence/summaryText.spec.ts`, `src/live-check/intelligence/voiceEvidence.spec.ts`, `src/live-check/intelligence/voiceTargets.spec.ts`

**Persisted keys owned**: `livestream-intelligence`, `livestream-feedback` (= alertId),
`livestream-diagnostics`, `livestream-intelligence-event` (prune oldest 250 above 3,000);
voiceprint JSON file (`LIVESTREAM_DESTINY_VOICEPRINT_PATH`, format v1).

**AGENTS.md invariants**: viewer surges compare against a 5-20-minute-old baseline that
must be flat and not still climbing after go-live; platform surges must also reach the
median peak of recent full sessions (>=3 sessions of >=30 min in 30 d); sparse baselines
after restart or primary switch suppress rather than alert; two consecutive candidates;
`destiny_guest` and `viewer_surge` at most once per session; 30-min cooldown rolled back
on delivery failure; monthly budget from `cost-event` where
`feature == "livestream-intelligence"`.

**Side effects to stub**: yt-dlp and ffmpeg (fake executables on PATH via
`YT_DLP_PATH`/`FFMPEG_PATH` in tests), sherpa models (trait `SpeechEngine` with fake),
AI model, Pushover.

**Foundation APIs**: `Ai::generate_object` (`ModelRole::LivestreamIntelligence`,
`maxOutputTokens` 700), `CostRecorder` (transcription, self-hosted), entities, `Pushover`,
`spawn_tracked`, bounded subprocess helper (WP00 provides
`omni_core::process::run_bounded(cmd, stdout_cap, stderr_cap, timeout)`).

**Implements ports**: `LiveIntelligence`.
**Routes**: `GET /api/streamers/:id/intelligence-details`,
`POST /api/streamers/:id/intelligence-feedback` (404 `Unknown streamer` via
`LiveDirectory`). **Tasks**: none (driven by `after_tick`). **MCP tools**: none.
**Spike**: sherpa-onnx crate linking + embedding parity (>= 0.62 on known clips).

---

## WP06 PressPods

**Crate**: omni-presspods (routes, submit, task, pipeline, persistence, url normalize,
retrievers {postlight→dom_smoothie+cleanup, readability, extractus→scraper, wayback,
removepaywall, fetch, jina, x}, formatting, agents {metadata, cleaner}, speech {higgs,
elevenlabs, stt, coverage, chunking, synthesize}, audio_chain, storage, id3, rss, costs,
MCP tools).

**TS files**: `src/press-pods/**`, `src/mcp/tools/press-pods.ts`,
`src/types/postlight-parser.d.ts`, `src/tools/tts-bakeoff.ts` (dropped: throwaway tool;
recorded as not ported).

**Spec files to port**: `src/press-pods/agents/parsing.spec.ts`, `src/press-pods/errors.spec.ts`, `src/press-pods/formatting/lineFiltering.spec.ts`, `src/press-pods/persistence.spec.ts`, `src/press-pods/pipeline.effect.spec.ts`, `src/press-pods/publicHttp.spec.ts`, `src/press-pods/retrievers/index.spec.ts`, `src/press-pods/retrievers/jina.spec.ts`, `src/press-pods/retrievers/x.spec.ts`, `src/press-pods/routes.spec.ts`, `src/press-pods/rssText.spec.ts`, `src/press-pods/speech/audioChain.effect.spec.ts`, `src/press-pods/speech/coverage.spec.ts`, `src/press-pods/speech/textChunking.spec.ts`, `src/press-pods/storage.spec.ts`, `src/press-pods/submit.spec.ts`, `src/press-pods/url.spec.ts`; plus RSS golden (`tests/golden/rss.xml` byte
comparison after normalizing `lastBuildDate`) and ffmpeg argument golden (every filter
string, `RESAMPLE_HQ` on every `aresample`, `FIZZ_SHELF` on the Higgs denoise path).

**Persisted keys owned**: `press-pods-episode` (episodeId base64url 16 bytes),
`press-pods-job` (durable queue); files `<audioDir>/<id>.mp3`,
`<audioDir>/.chunks/<workId>/<key>.wav`.

**AGENTS.md invariants**: retrievers run independently (concurrency 7, 60 s each) and the
metadata model selects the best usable article; speech chunked and verified before the
finished episode is exposed (file first, then row, file deleted if row fails); every
sample-rate conversion keeps `RESAMPLE_HQ`; Higgs denoise keeps `FIZZ_SHELF` and `arnndn`
with `assets/press-pods/denoise.rnnn` (doctor check); job retries 6 with 60 s x 2^(n-1),
stale claim 30 min, boot resets `processing` claims; read `docs/presspods-audio.md` first.

**Side effects to stub**: retriever HTTP (public client), fxtwitter, Jina, Wayback,
removepaywall, Higgs/ElevenLabs TTS, STT, ffmpeg/ffprobe (fake binaries for unit tests;
one `#[ignore]` real-ffmpeg test), Karakeep bookmark, Pushover, AI model.

**Foundation APIs**: `PublicHttpClient`, `Ai` (`PressPodsMetadata`, `PressPodsCleaning`),
`CostRecorder` (tts, retrieval, transcription), entities, `TaskRegistry::run_now`,
`server-kit` (byte ranges implemented locally), `ct_eq_sha256`, `process::run_bounded`.

**Routes**: `POST /pods/episodes`, `GET|HEAD /pods/rss`, `GET|HEAD /pods/audio/:file`,
`GET /pods/logo.jpeg`, `GET /api/press-pods/episodes`, `GET|DELETE /api/press-pods/episodes/:id`,
`POST /api/press-pods/episodes/:id/retry`, `POST /api/press-pods/submit`,
`POST /api/press-pods/jobs/:jobId/retry`, `DELETE /api/press-pods/jobs/:jobId`.
**Tasks**: `PressPods`. **MCP tools**: `presspods_list`, `presspods_episode_get`,
`presspods_transcript_read`, `presspods_submit`, `presspods_retry`, `presspods_delete`.

---

## WP07 Podcast recommendations + Castro

**Crate**: omni-podcasts (castro {auth HMAC, api, client, protocol, fractional indexing},
podcastindex, itunes, rss reader, candidates, filters, voices, shortlist, selection, guest
selection, discovery, pipeline, outcomes, reflection, taste, persistence, account,
subscriptions, cleanup task, Castro alert gate, routes, MCP tools, `#[ignore]` castro smoke
test).

**TS files**: `src/podcast-recs/**`, `src/alerts/castro*`, `src/mcp/tools/podcasts.ts`,
`src/tools/castro-smoke.ts`.

**Spec files to port**: `src/alerts/castro.spec.ts`, `src/podcast-recs/candidates.spec.ts`, `src/podcast-recs/castro/api.spec.ts`, `src/podcast-recs/castro/auth.spec.ts`, `src/podcast-recs/castro/client.spec.ts`, `src/podcast-recs/castro/inboxCleanupTask.spec.ts`, `src/podcast-recs/castro/protocol.spec.ts`, `src/podcast-recs/filters.spec.ts`, `src/podcast-recs/guestSelection.spec.ts`, `src/podcast-recs/itunes.spec.ts`, `src/podcast-recs/outcomes.spec.ts`, `src/podcast-recs/persistence.spec.ts`, `src/podcast-recs/pipeline.spec.ts`, `src/podcast-recs/podcastindex/auth.spec.ts`, `src/podcast-recs/podcastindex/client.spec.ts`, `src/podcast-recs/reflection/reflection.spec.ts`, `src/podcast-recs/rss.spec.ts`, `src/podcast-recs/task.spec.ts`, `src/podcast-recs/taste.spec.ts`, `src/podcast-recs/types.spec.ts`, `src/podcast-recs/voices.spec.ts`; plus golden vectors for rocicorp
`generateKeyBetween` captured from node.

**Persisted keys owned**: `podcast-recommendation-attempt`, `podcast-run-state`
(`singleton`, `voiceCursor`), `podcast-taste-evidence` (`listen:<digest>`,
`recommendation:<digest>`), `podcast-taste-profile` (fingerprint guard).

**AGENTS.md invariants**: Castro inbox preview cleanup every six hours (doc drift in
`docs/castro-sync.md` to fix), clears only descriptions starting exactly
`"This is a free preview"`; failed runs stay visible but Pushover is gated on >= 3
consecutive failures spanning >= 12 h minus 5 min from durable run history; success resets;
isolated 500/socket failures never notify; commit sequence pending row → Castro enqueue →
`queueResult` → notify → `notified`; three-state results never empty-on-failure.

**Side effects to stub**: Castro (`tentacles.castro.fm`), Podcast Index, iTunes, RSS feeds,
Tavily, AI models, Pushover, markdown run log file under `LOGS_PATH/podcast-recs/`.

**Foundation APIs**: `Ai` (shortlist/selection/discovery/guests/reflection), `WebSearch`,
`HttpClient`, entities, `fingerprint_evidence`, `digest`, `TaskRegistry::recent_runs`
(gate), `AlertGate`, `Pushover` (`Podcast`), `typed_tool`.

**Routes**: `GET /api/podcast-recommendations`, `GET /api/podcast-recommendations/taste-profile`,
`GET /api/podcast-recommendations/:id`, `POST /api/podcast-recommendations/:id/feedback`,
`POST /api/podcast-recommendations/run`. **Tasks**: `PodcastRecs`, `PodcastTasteReflection`,
`CastroInboxCleanup`. **MCP tools**: `podcast_account_list`, `podcast_account_search`,
`podcast_account_update`, `podcast_recommendations_list`, `podcast_recommendation_get`,
`podcast_recommendation_feedback`, `podcast_taste_read`.

---

## WP08 Media recommendations

**Crate**: omni-media (plex, tmdb, arr {radarr, sonarr}, identity, history, candidates,
shortlist, selection, filters, media library, pipeline, outcomes, taste {evidence,
reflection, profile}, persistence, on-deck, feedback URL, routes, MCP tools).

**TS files**: `src/recommendations/**`, `src/mcp/tools/media.ts`, `src/mcp/tools/media-shared.ts`.

**Spec files to port**: `src/recommendations/arr/radarr.spec.ts`, `src/recommendations/arr/sonarr.spec.ts`, `src/recommendations/candidates.enrichment.spec.ts`, `src/recommendations/candidates.spec.ts`, `src/recommendations/effect.spec.ts`, `src/recommendations/filters.spec.ts`, `src/recommendations/identity.spec.ts`, `src/recommendations/mediaLibrary.spec.ts`, `src/recommendations/outcomes.spec.ts`, `src/recommendations/persistence.spec.ts`, `src/recommendations/pipeline.spec.ts`, `src/recommendations/plex/client.spec.ts`, `src/recommendations/shortlist.spec.ts`, `src/recommendations/task-startup.spec.ts`, `src/recommendations/taste/taste.spec.ts`, `src/recommendations/tmdb/types.spec.ts`

**Persisted keys owned**: `recs-recommendation-attempt`, `recs-identity-alias` (negative
lookups cached), `recs-taste-evidence`, `recs-taste-profile`.

**AGENTS.md invariants**: notifications idempotent (`notificationState` reserved before
Pushover, `reserved` reconciles to `unknown`, never resent); unavailable Plex history,
library or watchlist aborts the run; 180-day cooldown, failed retry after 24 h; external
writes (Radarr/Sonarr add) reported only after acceptance.

**Side effects to stub**: Plex, TMDB, Radarr, Sonarr, Tavily, AI models, Pushover.

**Foundation APIs**: `Ai` (`RecsShortlist`, `RecsSelection`, `TasteReflection`),
`WebSearch`, `HttpClient`, entities, `fingerprint_evidence`, `TaskRegistry`,
`Pushover` (`Recs`), `typed_tool`. **Implements ports**: `OnDeckSource`.

**Routes**: `GET /api/recommendations`, `GET /api/recommendations/taste-profile`,
`GET /api/recommendations/:id`, `POST /api/recommendations/:id/feedback`,
`POST /api/recommendations/run`. **Tasks**: `Recommendations`, `TasteReflection`.
**MCP tools**: `media_catalog_search`, `media_catalog_get`, `media_catalog_browse`,
`media_library_list`, `media_watchlist_list`, `media_watchlist_add`,
`media_recommendations_list`, `media_recommendation_get`, `media_recommendation_feedback`,
`media_taste_read`.

---

## WP09 Arr recovery + Observer repair

**Crate**: omni-arr (modules `arr_recovery` {client, policy, nzbget, filesystem, llm,
service, persistence, task}, `observer` {client}, `observer_repair` {arr, agent, service,
persistence, task}).

**TS files**: `src/arr-recovery/**`, `src/observer/**`, `src/observer-repair/**`.

**Spec files to port**: `src/arr-recovery/boundaries.spec.ts`, `src/arr-recovery/client.spec.ts`, `src/arr-recovery/filesystem.spec.ts`, `src/arr-recovery/llm.spec.ts`, `src/arr-recovery/nzbget.spec.ts`, `src/arr-recovery/persistence.spec.ts`, `src/arr-recovery/policy.spec.ts`, `src/arr-recovery/service.spec.ts`, `src/observer-repair/agent.spec.ts`, `src/observer-repair/arr.spec.ts`, `src/observer-repair/persistence.spec.ts`, `src/observer-repair/service.spec.ts`, `src/observer/client.test.ts`

**Persisted keys owned**: `arr-recovery-state` (pk `kind`, lease 25 min),
`observer-repair-state` (pk numeric `issueId` → `n<id>`, lease 30 min, `revision` =
sha256 of JS JSON first 24 hex).

**AGENTS.md invariants**: `ArrRecovery` requires an unchanged explicit import failure
across observations spanning 15 minutes, never bypassed for manual runs; Luna interprets
names but cannot override exact target mappings or unsafe import rejections; mutations
reserved durably, imports/deletions verified before success; search only missing monitored
targets with durable retry limits (6 h backoff, 3 per 7 days); ObserverRepair preserves
exact TMDB/TVDB identity and file→import→grab provenance and the report scope ceiling;
never repeats uncertain deletions/searches; resolve only after search acceptance and a
verified explanatory comment (`[Omni repair <id>/<rev>]`), then Pushover; unsupported
issues stay open and notify once. Read `docs/arr-recovery.md`, `docs/observer-repair.md`.

**Side effects to stub**: Sonarr/Radarr API, NZBGet JSON-RPC, local filesystem checks
(temp dirs), Overseerr API, AI models (agent tool loop), Pushover.

**Foundation APIs**: `Ai::generate_object` (maxRetries 0, 60 s), `Ai::run_tool_loop`
(16 steps), entities + leases via `write`, `HttpClient`, `Pushover` (`Recs` fallback
General), `TaskRegistry`.

**Routes**: none. **Tasks**: `ArrRecovery`, `ObserverRepair`. **MCP tools**: none.

---

## WP10 Server iCloud Reminders

**Crate**: omni-reminders (config, store (AES-GCM private file), apple (GSA SRP, 2FA,
PCS via `protected_access`), cloudkit, cloudkit_extras, codec (topotext protobuf), recurrence,
recurring_completion, service (ledger, index), routes, MCP tools).

**TS files**: `src/reminders/**`, `src/icloud/**`, `src/mcp/tools/reminders.ts` (+ spec).

**Spec files to port**: `src/icloud/protectedAccess.spec.ts`, `src/mcp/tools/reminders.spec.ts`, `src/reminders/apple.spec.ts`, `src/reminders/cloudkit.spec.ts`, `src/reminders/cloudkitExtras.spec.ts`, `src/reminders/codec.spec.ts`, `src/reminders/recurrence.spec.ts`, `src/reminders/recurringCompletion.spec.ts`, `src/reminders/routes.spec.ts`, `src/reminders/service.spec.ts`, `src/reminders/store.spec.ts`; plus golden decrypt of a store file and
tough-cookie JSON round trip produced by the TS code.

**Persisted state owned**: `/data/reminders-private/<sha256(lower(account))>.enc`
(format `0x01|iv12|tag16|ct`, AAD `omni-reminders:v1:<identity>`, dir 0700, file 0600,
O_NOFOLLOW, <= 4 MiB, atomic write + fsync); no docstore keys.

**AGENTS.md invariants**: independent of Mac EventKit and IMAP/CalDAV; disabled without
complete `ICLOUD_REMINDERS_*` (never fails boot); HTTPS code page has no extra login, only
bounded auth controls and public status; data and CRUD only behind MCP bearer auth; strict
Origin/Host/Sec-Fetch-Site checks, JSON content type, rate limits; serialized challenges;
private encrypted session storage; durable mutation reservations and read-after-write
verification; never accept Apple terms or disable ADP; recurrence rule changes need exact
parent and rule tags, atomic linkage, fresh verification; unknown or multiple rules stay
protected; recurring completion uses the dedicated one-shot query, durable reservation,
fresh verification of original and completed occurrence, never replayed; ordinary recurring
edits blocked; list creation preserves the Account ordering CRDT; Apple client keeps the
approved ioBroker User-Agent/Referer exception (only here); no automatic sign-in. Read
`docs/server-reminders.md`.

**Side effects to stub**: idmsa.apple.com, setup.icloud.com, CloudKit endpoints (wiremock
with recorded fixtures), Pushover, filesystem (tempdir with permission checks).

**Foundation APIs**: `HttpClient::raw` with custom headers (UA exception documented),
`cookie_store`, `aes-gcm`, `Pushover`, `server-kit` (`JsonBody` 512 B variant, `api_error`),
`typed_tool`, `TaskRegistry`.

**Routes**: `GET /api/reminders/status`, `POST /api/reminders/auth/start`,
`POST /api/reminders/auth/code`, `POST /api/reminders/auth/verify` (guards per
ARCHITECTURE.md 6). **Tasks**: `RemindersSession`. **MCP tools**: `list_reminder_lists`,
`get_reminder_list`, `update_reminder_list`, `get_reminder_recurrence`,
`create_reminder_recurrence`, `update_reminder_recurrence`, `remove_reminder_recurrence`,
`complete_recurring_reminder`, `list_reminders`, `get_reminder`, `create_reminder`,
`update_reminder`, `complete_reminder`, `reopen_reminder`, `delete_reminder`.

---

## WP11 Workspaces + Briefings

**Crates**: omni-workspaces (types, definitions (instruction strings verbatim), persistence
+ `apply_workspace_transaction`, engine (plan-then-commit), actions, task, email ingestion
handler, notifications, schema, routes, MCP tools), omni-briefings (configs + front matter,
placeholders, persistence, agent task, `BriefingsReader`, route).

**TS files**: `src/workspaces/**`, `src/briefing-agent/**`, `src/mcp/tools/workspaces.ts`.

**Spec files to port**: `src/briefing-agent/configs.spec.ts`, `src/briefing-agent/persistence.spec.ts`, `src/briefing-agent/placeholders.spec.ts`, `src/workspaces/actions.test.ts`, `src/workspaces/definitions.test.ts`, `src/workspaces/email.test.ts`, `src/workspaces/emailHandler.test.ts`, `src/workspaces/engine.test.ts`, `src/workspaces/notifications.test.ts`, `src/workspaces/persistence.test.ts`, `src/workspaces/schema.test.ts`, `src/workspaces/task.test.ts`

**Persisted keys owned**: `workspace-subject` (`workspaceId, subjectId`),
`workspace-email-scope`, `workspace-artifact-revision`, `workspace-message`,
`workspace-source` (email ids `email:<ws>:<subject>:<emailId>`), `workspace-action`
(`payload` JSON string compared byte-for-byte: key order `senders, domains,
subjectKeywords, bodyKeywords`), `workspace-papercut` (fingerprint
`ws:category:relatedTool:title.trim().lower()`), `workspace-notification`;
`briefing-history`, `briefing-delivery` (`briefingName, deliveryId`).

**AGENTS.md invariants**: workspace rows change through the service/API only; pending
actions, Marketplace publishing, buyer messages, offers, address disclosure and meetups
require user authorization (approve endpoint/tool), research and drafting never authorize
them; plan and validate all model output before any write, then one transaction; user
message persisted before the model runs; notifications at most once (`sending` found on
retry → `unknown`, never resent), backoff min(5 min x 2^k, 6 h); in-process approval mutual
exclusion; deterministic CalDAV UID `workspace-<actionId>@omni-notify` with 412 = created;
email sources persisted before trigger, `triggeredAt` only after success; briefing
deliveries reserved before send and released on failure; OpenAI strict schema without
`oneOf`.

**Side effects to stub**: AI models (tool loop with `web_search`, `fetch_url`,
`report_papercut`, `send_notification`), Tavily, page fetch, Pushover (`Workspace`,
`Briefing`), `CalendarWriter` port, markdown log files.

**Foundation APIs**: `Ai::run_tool_loop` (12 / 20 steps, `reasoning_effort: high`),
`schema::strict_schema`, entities + `write`, `TaskRegistry::run_now_and_wait`,
`Pushover`, `typed_tool`, `JsonBody`, `serde_norway`, `CronSchedule::parse` (briefing front
matter validation). **Implements ports**: `EmailHandler` (`Workspaces`), `BriefingsReader`.

**Routes**: `GET /api/workspaces`, `GET /api/workspaces/:workspaceId`,
`GET /api/workspaces/:workspaceId/subjects/:subjectId`, `POST /api/workspaces/:workspaceId/messages`,
`POST /api/workspaces/:workspaceId/subjects/:subjectId/status`,
`POST /api/workspace-actions/:actionId/approve`, `POST /api/workspace-actions/:actionId/reject`,
`GET /api/workspace-papercuts`, `POST /api/workspace-papercuts/:papercutId/resolve`,
`GET /api/briefings`. **Tasks**: `PurchaseResearch`, `MarketplaceSelling`,
`WorkspaceNotifications`, one per briefing config. **MCP tools**: `workspaces_list`,
`workspace_get`, `workspace_search`, `workspace_message`, `workspace_subject_set_status`,
`workspace_actions_list`, `workspace_action_approve`, `workspace_action_reject`,
`workspace_papercuts_list`, `workspace_papercut_resolve`.

---

## WP12 MCP server, MCP Events, Device link

**Crates**: omni-mcp (auth layer, rmcp service mount, per-request server, activity
recording + prune + `markInterruptedCalls`, activity routes, policy inventory, events
{protocol pre-router, catalog, service (outbox), persistence, webhook, executor auth,
claude session watcher}, system/events/claude-session tools, tool ordering + golden
check), omni-device-link (long-poll service, routes, execute state machine). Also owns
`xtask mcp-policy` logic (ported from `src/tools/generate-mcp-policy.ts`; WP00 provides the
xtask shell).

**TS files**: `src/mcp/{activity,activityRoutes,auth,policy,route,runtime,server,tool}.ts`
(+ specs), `src/mcp/events/**`, `src/mcp/tools/{system,events,claude-sessions,index}.ts`
(+ spec), `src/device-link/**`, `src/tools/generate-mcp-policy.ts`.

**Spec files to port**: `src/device-link/routes.spec.ts`, `src/device-link/service.spec.ts`, `src/mcp/activity.spec.ts`, `src/mcp/auth.spec.ts`, `src/mcp/events/claudeSessions.spec.ts`, `src/mcp/events/executorAuth.spec.ts`, `src/mcp/events/protocol.spec.ts`, `src/mcp/events/service.spec.ts`, `src/mcp/events/webhook.spec.ts`, `src/mcp/policy.spec.ts`, `src/mcp/server.spec.ts`, `src/mcp/tool.spec.ts`, `src/mcp/tools/claude-sessions.spec.ts`; plus: `tools/list` golden for both eras (legacy
`2025-*`/`2024-*` stateless streamable HTTP, modern `2026-07-28` incl. `server/discover`
advertising `events`), and the executor adapter `node --test` suite against the Rust binary.

**Persisted keys owned**: `mcp-call` (prune to 2000 above 2100), `mcp-event-subscription`
(legacy `folder` rows decode to `arguments`), `mcp-event-receipt`, `mcp-event-delivery`,
`mcp-event-request` (newest 30), `mcp-claude-session-watch`.

**AGENTS.md invariants**: MCP tools are bounded adapters over existing services with
strong bearer validation (digest + constant time, 401 before MCP handling, `no-store` +
`nosniff`, 503 when unconfigured); tool names/titles/descriptions/annotations/schemas and
`docs/mcp-policy.json` byte-compatible; tool failures are `isError` results; MCP Events
share one outbox, polling tools remain; never hold the outbox lock across webhook or
authorization I/O; deliver only with a stored delegated token that validates at delivery
time; `refreshBefore` no later than token expiry, `withheld` until refresh or end, never
extend access; receipts committed atomically with deliveries before the IMAP cursor commits
(handler registered first); key/id derivations bit-identical; device link: Claude Code
session tools reach the host only through the outbound long poll with
`OMNI_DEVICE_LINK_TOKEN` (never equal to or substituting for the MCP token); withdraw jobs
not picked up (30 s); never retry a delivered job with unknown outcome; newer poll wins;
45 s online window; session starts limited to the host's project list (enforced on the
host); tool text, results and errors never mention Mac, macOS or the hostname
(`scrubHostDetails`, machine-word lint over golden schemas); no permission mode added.
Read `docs/mcp.md`, `docs/mcp-events.md`, `docs/claude-sessions.md`.

**Side effects to stub**: webhook receivers (wiremock, public-address guard overridden in
tests through `PublicHttpClient` test resolver), Executor auth endpoint, device-link host
(test client driving `/device-link/poll` and `/result`).

**Foundation APIs**: `omni-mcp-kit` (all), `server-kit::bearer_digest_eq`, `PublicHttpClient`,
entities + `write`, `TaskRegistry` (system tools), `aes-gcm`/`hmac`, `ports::{LiveDirectory,
LiveIntelligence, BriefingsReader, ArchiveEcho}`. Collects every subsystem's `McpTool`s from
WP14 in TS order (`reminders, system, workspaces, email, calendar, email-compose,
email-archive, events, email-attachments, media, podcasts, press-pods, personal, printer,
browser-history, claude-sessions`).

**Routes**: `ALL /mcp`, `POST /device-link/poll`, `POST /device-link/result`,
`GET /api/mcp/activity`, `GET /api/claude/activity`, `GET /api/claude/sessions`,
`GET /api/claude/sessions/:session/transcript`, `GET /api/claude/projects`.
**Tasks**: `McpEventDelivery`, `ClaudeSessionEvents`. **Services**: delivery worker
(sliding wake queue), boot drain. **Email handler**: `McpEvents` (first).
**MCP tools**: `system_status`, `tasks_list`, `task_run`, `task_runs_list`, `task_run_get`,
`livestreams_list`, `livestream_get`, `briefings_list`, `events_status`,
`claude_link_status`, `claude_sessions_list`, `claude_session_get`, `claude_session_read`,
`claude_session_start`, `claude_session_send`, `claude_session_stop`.
**Spike (day 1)**: rmcp 3.4 custom `events/*` methods + `capabilities.events` + header
access, top-level `oneOf` input schemas; fallback axum pre-router.

---

## WP13 Personal services

**Crate**: omni-personal (modules `pets` {auth (Cognito SRP), api, math, persistence
(relational tables), task, seed (dev bin `omni-pets-seed`)}, `printer` {service, IPP,
cupsfilter/rastertobrlaser pipeline}, `hister` {service}, `reset_alerts` {source,
delivery, presentation, task}, `codex_resets` {source, history, policy, delivery, task},
`claude_resets` {source, policy, task}, routes, MCP tools).

**TS files**: `src/pet-tracker/**`, `src/printer/**`, `src/hister/**`,
`src/reset-alerts/**`, `src/codex-resets/**`, `src/claude-resets/**`,
`src/mcp/tools/{personal,printer,browser-history}.ts`.

**Spec files to port**: `src/claude-resets/policy.spec.ts`, `src/claude-resets/source.spec.ts`, `src/codex-resets/delivery.spec.ts`, `src/codex-resets/history.spec.ts`, `src/codex-resets/policy.spec.ts`, `src/hister/service.spec.ts`, `src/pet-tracker/api.spec.ts`, `src/pet-tracker/task.spec.ts`, `src/printer/service.spec.ts`, `src/reset-alerts/delivery.spec.ts`, `src/reset-alerts/task.spec.ts`

**Persisted keys owned**: tables `pets`, `pet_weight_history`; `printer-accepted-job`
(sha256 of pdf bytes + JS JSON of normalized options); `codex-reset-delivery`,
`claude-reset-delivery` (`DEFAULT_TTL_MS` 90 days + `validate`; raw writes set
`expires_at = now + 90 d`).

**AGENTS.md invariants**: CodexResets polls Reset Beacon alerts + history every minute;
predictions, announcements, observed rollouts and reported landings stay distinct; never
infer a landing from an elapsed deadline or a banked grant from an ordinary reset;
completed history may precede the alert feed, dedup by source post and event; concise
pushes with a source button; durable delivery reservations (sending = uncertain, skip; 4xx
deletes and fails run; disabled Pushover fails run); keys use raw source strings, never
re-serialized timestamps; ClaudeResets reads confirmed historical counter-reset reports,
tracker confirmation is not account verification, banked-reset wording without inferring
redemption or expiry, separate durable namespace, Codex keys unchanged; Hister text and
listings bounded (UTF-16 offsets), page content untrusted, label writes verified by
read-back, upstream token never exposed or forwarded through redirects; printer duplicate
suppression (5 min, in-flight + durable), result tells caller not to retry when the
durable write failed. Read `docs/codex-resets.md`, `docs/claude-resets.md`.

**Side effects to stub**: Cognito + Whisker GraphQL, IPP printer (fake IPP server or
trait), `pdfinfo`/`cupsfilter`/`rastertobrlaser` (fake binaries), document download, Hister
API, resetbeacon.com, resetradar.com, Pushover.

**Foundation APIs**: `Store::table`, entities with TTL, `HttpClient`, `PublicHttpClient`
(print document), `Pushover` (`General`), `process::run_bounded`, `typed_tool`, `TaskRegistry`.

**Routes**: `GET /api/pets`, `GET /api/pets/:petId/export.csv`. **Tasks**: `PetTracker`,
`CodexResets`, `ClaudeResets`. **MCP tools**: `pets_read`, `costs_read` (calls
`omni_ai::costs`), `get_printer_status`, `print_document`, `search_browser_history`,
`browse_browser_history`, `get_browser_page`, `set_browser_page_label`.

---

## WP14 App wiring, ops routes, data manager, cutover

**Crate**: omni-notify (bin): `main.rs` (CLI: default serve, `--server-only`,
`--run-task <Name>` case-insensitive outside the registry, `healthcheck`, `doctor`,
`compat-audit`; exit codes 130/1), `boot.rs` (order below), `wiring.rs` (builds every
`Subsystem`, sets ports, collects tasks/tools/entities/handlers/gates), `logging.rs`
(subscriber stack), `ops/` (health, tasks, task-runs, run logs + SSE, snapshot + dashboard
SSE hub, costs), `data_manager.rs`, `preview.rs` (fixture mode replacing
`src/tools/preview-server.ts`: `omni-notify --preview` serves fake data for frontend work).

**TS files**: `src/index.ts`, `src/server.ts`, `src/data-manager.ts` (+ spec),
`src/tools/preview-server.ts`.

**Spec files to port**: `src/data-manager.spec.ts`; plus dashboard SSE tests (fresh initial snapshot,
150 ms debounce, identical-payload skip, 25 s ping, monotonically increasing ids), run-log
stream (`init`/`line`/`done`, immediate `done` for finished runs), route-precedence test
(`/pods/rss` vs SPA `/pods`), boot-order test with stub subsystems.

**Boot order** (matches `src/index.ts`): config + redacted log → open store → `migrate_all`
over all `EntityDescriptor`s → `import_historical_costs` → subsystem construction
(intelligence, channels.json, iOS controls, tasks, Reminders) → `registry.initialize`
(interrupted runs) → `markInterruptedCalls` → events service + device link → start HTTP
server → register internal tasks → email features (background, retry 30 s..300 s) →
scheduler start → catch-up recovery (tracked task).

**Persisted keys owned**: none new; data manager reads all managed entities (slug = entity
name) and `PRAGMA page_count * page_size`; delete hooks (`task-run` blocked while running,
cascades to `task-run-log`; `email-activity` cascades to `email-activity-log`).

**AGENTS.md invariants**: all of the above wired without weakening; `/api/*` same-origin
mutation guard; LAN-only API without session auth (unchanged); prefer committed boot
migrations for data changes; update AGENTS.md at cutover (pnpm commands → cargo gates,
Effect section → Rust conventions, architecture seams → crate list).

**Side effects to stub**: everything, via `TestApp`.

**Routes**: `GET /api/health`, `GET /api/tasks`, `POST /api/tasks/:name/run`,
`GET /api/task-runs`, `GET /api/task-runs/:runId/logs`, `GET /api/task-runs/:runId/logs/stream`,
`GET /api/snapshot`, `GET /api/events`, `GET /api/costs`, `GET /api/data/entities`,
`GET|DELETE /api/data/entities/:slug`, `/reminders` page headers, SPA fallback.
**Tasks**: `StoreMaintenance`. **Cutover runbook**: ARCHITECTURE.md 4.5 steps 1-6, Dockerfile
and CI switch (section 8), delete TS sources only after one week of stable production.

---

## WP15 Frontend shell, kit and ops pages

**Crates**: omni-web-kit (lib), omni-web (trunk bin: `index.html`, `Trunk.toml`,
`style/index.css` verbatim, `src/main.rs`, `src/app.rs` router + shell, ops pages).

**TS files**: `frontend/src/` except the WP16 set: `main.tsx`, `App.tsx`, `router.tsx`,
`live.tsx`, `api.ts` (client functions; DTOs come from `omni-api` written by backend
packages), `effect.ts`, `vite-env.d.ts` (dropped), `hooks/*`, `utils/*`, all `components/*`
except `RecommendationRuns.tsx` and `TasteBrain.tsx`, pages `Home`, `Operations`, `Data`,
`Costs`, `EmailActivity`, `Streamer`, `LivestreamIntelligence`.

**Spec files to port**: `frontend/src/api.spec.ts`, `frontend/src/components/WorkspaceMarkdown.spec.tsx`, `frontend/src/effect.spec.ts`, `frontend/src/utils/claudeActivity.spec.ts` (`api.spec.ts` becomes DTO round-trip tests in
`omni-api` against golden fixtures; `effect.spec.ts` becomes retry-policy tests of the
gloo client; `WorkspaceMarkdown.spec.tsx` as `wasm-bindgen-test`; `claudeActivity.spec.ts`
native).

**Contract**: routes and deep links (`/`, `/media`, `/recommendations` → `/media`,
`/media/:id`, `/podcasts`, `/podcasts/:id`, `/feedback/(recommendations|podcasts)/:id`,
`/pods`, `/pods/:id`, `/streamers/:id`, `/streamers/:id/intelligence`, `/briefings`,
`/emails`, `/data`, `/costs`, `/workspaces[/:w[/:s]]` with `?section&target`,
`/operations`, `/reminders`, `/pets`, `/mcp-activity`, `/claude`, 404); live data behavior
(ARCHITECTURE.md 7); CSS class names unchanged; `/reminders` runs under the CSP with
`'wasm-unsafe-eval'` and no inline script.

**Side effects to stub**: backend (`omni-notify --preview` fixture server or mocked fetch
in wasm tests). **Foundation APIs**: `omni-api` DTOs only.
**Provides to WP16**: `omni_web_kit::{api, live::use_live_data, hooks, components, charts,
markdown::WorkspaceMarkdown, utils}`; WP16 exports `omni_web_pages::{MediaPage,
MediaDetailPage, PodcastsPage, PodcastDetailPage, FeedbackPage, PodsPage, PodsDetailPage,
WorkspacesPage, PetsPage, RemindersPage, BriefingsPage, McpPage, ClaudePage}`, which
`omni-web`'s router mounts.

---

## WP16 Frontend domain pages

**Crate**: omni-web-pages.

**TS files**: `frontend/src/pages/{RecommendationsPage, RecommendationDetailPage,
PodcastsPage, PodcastDetailPage, PodsPage, PodsDetailPage, WorkspacesPage, PetsPage,
RemindersPage(+spec), BriefingsPage, FeedbackPage, ClaudePage, McpPage}.tsx`,
`frontend/src/components/{RecommendationRuns,TasteBrain}.tsx`.

**Spec files to port**: `frontend/src/pages/RemindersPage.spec.tsx` (Reminders state machine through a
`RemindersTransport` seam; fetch options `credentials: omit`, `cache: no-store`,
`redirect: error`).

**Contract**: same endpoints as today (see each page in the frontend survey), audio chapter
seek, pets CSV link, `?recommendation=` highlight, `?section=&target=` deep-link scroll
and highlight, 409 handling for "already running".

**Side effects to stub**: backend. **Foundation APIs**: `omni-api`, `omni-web-kit`.

---

## WP17 Executor events adapter (required)

**Crate**: omni-events-adapter (bin). Required: the goal removes all TypeScript and pnpm
tooling, so the Node package is replaced by a Rust binary with the same contract and image
name, and its tests are ported to Rust.

**TS files**: `packages/executor-events-adapter/src/{auth,continuations,legacy,server}.ts`,
`packages/executor-events-adapter/test/*.node-test.mjs`.

**Spec files to port**: `packages/executor-events-adapter/test/adapter.node-test.mjs`, `packages/executor-events-adapter/test/native.node-test.mjs`

**Persisted keys**: none (in-memory continuations). **Contract**: `/mcp` on :4789,
`GET /health`, modern-era detection, `-32020`/`-32022`/`-32001` errors, events forwarding
headers (`x-omni-events-owner`, `x-omni-events-authorization`), continuations (5 min, one
shot, owner bound), `resultType: "complete"` and serverInfo meta. Image
`ghcr.io/micthiesen/executor-events-adapter:latest` and `deploy/executor-events/` unchanged.
**Side effects to stub**: Executor (`/mcp`, `/api/auth/mcp/get-session`), Omni `/mcp`.

---

## Appendix: assignment rules

First match wins (regex over repository-relative paths):

```
WP01 ^src/(email/imap/|email/archive/|mcp/tools/email-(compose|archive|attachments)(\.spec)?\.ts$)
WP02 ^src/(email/[^/]+$|parcel-tracker/|mcp/tools/email(-reprocess(\.spec)?)?\.ts$)
WP03 ^src/(calendar-events/|mcp/tools/calendar\.ts$)
WP05 ^src/(live-check/intelligence/|types/sherpa-onnx-node\.d\.ts$|tools/(enroll-destiny-voice|livestream-intelligence-doctor)\.ts$)
WP04 ^src/(live-check/|ios-controls/)
WP06 ^src/(press-pods/|mcp/tools/press-pods\.ts$|types/postlight-parser\.d\.ts$|tools/tts-bakeoff\.ts$)
WP07 ^src/(podcast-recs/|alerts/castro|mcp/tools/podcasts\.ts$|tools/castro-smoke\.ts$)
WP08 ^src/(recommendations/|mcp/tools/media(-shared)?\.ts$)
WP09 ^src/(arr-recovery/|observer/|observer-repair/)
WP10 ^src/(reminders/|icloud/|mcp/tools/reminders(\.spec)?\.ts$)
WP11 ^src/(workspaces/|briefing-agent/|mcp/tools/workspaces\.ts$)
WP13 ^src/(pet-tracker/|printer/|hister/|reset-alerts/|codex-resets/|claude-resets/|mcp/tools/(personal|printer|browser-history)\.ts$)
WP12 ^src/(mcp/|device-link/|tools/generate-mcp-policy\.ts$)
WP00 ^src/(effect/|utils/|types/|ai/|alerts/|task-runs/|costs/|emails/|test/)
WP14 ^src/(index\.ts|server\.ts|data-manager(\.spec)?\.ts|tools/preview-server\.ts)$
WP16 ^frontend/src/(pages/(Recommendation|Podcast|Pods|Workspaces|Pets|Reminders|Briefings|Feedback|Claude|Mcp)[A-Za-z]*(\.spec)?\.tsx|components/(RecommendationRuns|TasteBrain)\.tsx)$
WP15 ^frontend/src/
WP17 ^packages/executor-events-adapter/
```

## Coverage checklist (generated)

Generated with:

```sh
find src frontend/src packages/executor-events-adapter -type f \( -name '*.ts' -o -name '*.tsx' -o -name '*.mts' -o -name '*.mjs' \) -not -path '*/node_modules/*' -not -path '*/dist/*' | sort
```

615 files; 615 assigned; 0 unassigned. Each file appears exactly once below (first-match prefix rules in this document's appendix). `[spec]` marks a test file that must be ported as a Rust acceptance test.

### WP00 (51 files)

- [x] `src/ai/cost.ts`
- [x] `src/ai/registry.spec.ts` [spec]
- [x] `src/ai/registry.ts`
- [x] `src/ai/tools/fetchUrl.spec.ts` [spec]
- [x] `src/ai/tools/fetchUrl.ts`
- [x] `src/ai/tools/webSearch.spec.ts` [spec]
- [x] `src/ai/tools/webSearch.ts`
- [x] `src/alerts/throttle.spec.ts` [spec]
- [x] `src/alerts/throttle.ts`
- [x] `src/costs/migrate.ts`
- [x] `src/costs/persistence.ts`
- [x] `src/costs/summary.spec.ts` [spec]
- [x] `src/costs/summary.ts`
- [x] `src/effect/appRuntime.ts`
- [x] `src/effect/errors.spec.ts` [spec]
- [x] `src/effect/errors.ts`
- [x] `src/effect/http.spec.ts` [spec]
- [x] `src/effect/http.ts`
- [x] `src/effect/interop.spec.ts` [spec]
- [x] `src/effect/interop.ts`
- [x] `src/effect/publicHttp.spec.ts` [spec]
- [x] `src/effect/publicHttp.ts`
- [x] `src/effect/sse.spec.ts` [spec]
- [x] `src/effect/sse.ts`
- [x] `src/emails/client.spec.ts` [spec]
- [x] `src/emails/client.ts`
- [x] `src/emails/identity.ts`
- [x] `src/emails/mime.spec.ts` [spec]
- [x] `src/emails/mime.ts`
- [x] `src/emails/send.spec.ts` [spec]
- [x] `src/emails/send.ts`
- [x] `src/emails/templates.ts`
- [x] `src/task-runs/catchUp.spec.ts` [spec]
- [x] `src/task-runs/catchUp.ts`
- [x] `src/task-runs/events.spec.ts` [spec]
- [x] `src/task-runs/events.ts`
- [x] `src/task-runs/logCapture.spec.ts` [spec]
- [x] `src/task-runs/logCapture.ts`
- [x] `src/task-runs/persistence.spec.ts` [spec]
- [x] `src/task-runs/persistence.ts`
- [x] `src/task-runs/registry.spec.ts` [spec]
- [x] `src/task-runs/registry.ts`
- [x] `src/test/mitools.ts`
- [x] `src/types/turndown-plugin-gfm.d.ts`
- [x] `src/utils/config.spec.ts` [spec]
- [x] `src/utils/config.ts`
- [x] `src/utils/dates.ts`
- [x] `src/utils/feedbackUrl.ts`
- [x] `src/utils/fetchResult.ts`
- [x] `src/utils/fingerprint.spec.ts` [spec]
- [x] `src/utils/fingerprint.ts`

### WP01 (32 files)

- [x] `src/email/archive/persistence.ts`
- [x] `src/email/archive/service.spec.ts` [spec]
- [x] `src/email/archive/service.ts`
- [x] `src/email/imap/actions.spec.ts` [spec]
- [x] `src/email/imap/actions.ts`
- [x] `src/email/imap/archive.spec.ts` [spec]
- [x] `src/email/imap/archive.ts`
- [x] `src/email/imap/attachments.spec.ts` [spec]
- [x] `src/email/imap/attachments.ts`
- [x] `src/email/imap/autoRead.spec.ts` [spec]
- [x] `src/email/imap/autoRead.ts`
- [x] `src/email/imap/mapMessage.spec.ts` [spec]
- [x] `src/email/imap/mapMessage.ts`
- [x] `src/email/imap/persistence.ts`
- [x] `src/email/imap/readCache.spec.ts` [spec]
- [x] `src/email/imap/readCache.ts`
- [x] `src/email/imap/readCoherence.spec.ts` [spec]
- [x] `src/email/imap/sent.spec.ts` [spec]
- [x] `src/email/imap/sent.ts`
- [x] `src/email/imap/sync.spec.ts` [spec]
- [x] `src/email/imap/sync.ts`
- [x] `src/email/imap/transport.spec.ts` [spec]
- [x] `src/email/imap/transport.ts`
- [x] `src/email/imap/transportSent.spec.ts` [spec]
- [x] `src/email/imap/uidExpunge.spec.ts` [spec]
- [x] `src/email/imap/uidExpunge.ts`
- [x] `src/mcp/tools/email-archive.spec.ts` [spec]
- [x] `src/mcp/tools/email-archive.ts`
- [x] `src/mcp/tools/email-attachments.spec.ts` [spec]
- [x] `src/mcp/tools/email-attachments.ts`
- [x] `src/mcp/tools/email-compose.spec.ts` [spec]
- [x] `src/mcp/tools/email-compose.ts`

### WP02 (45 files)

- [x] `src/email/activity.spec.ts` [spec]
- [x] `src/email/activity.ts`
- [x] `src/email/activityLogs.spec.ts` [spec]
- [x] `src/email/activityLogs.ts`
- [x] `src/email/dispatcher.spec.ts` [spec]
- [x] `src/email/dispatcher.ts`
- [x] `src/email/feedback.spec.ts` [spec]
- [x] `src/email/feedback.ts`
- [x] `src/email/htmlToText.ts`
- [x] `src/email/linkMetadata.spec.ts` [spec]
- [x] `src/email/linkMetadata.ts`
- [x] `src/email/persistence.ts`
- [x] `src/email/retry.effect.spec.ts` [spec]
- [x] `src/email/retry.spec.ts` [spec]
- [x] `src/email/retry.ts`
- [x] `src/email/retryTask.spec.ts` [spec]
- [x] `src/email/retryTask.ts`
- [x] `src/email/senderRules.spec.ts` [spec]
- [x] `src/email/senderRules.ts`
- [x] `src/email/triage.spec.ts` [spec]
- [x] `src/email/triage.ts`
- [x] `src/email/types.ts`
- [x] `src/email/watchdogTask.spec.ts` [spec]
- [x] `src/email/watchdogTask.ts`
- [x] `src/mcp/tools/email-reprocess.spec.ts` [spec]
- [x] `src/mcp/tools/email-reprocess.ts`
- [x] `src/mcp/tools/email.ts`
- [x] `src/parcel-tracker/carriers/candidates.spec.ts` [spec]
- [x] `src/parcel-tracker/carriers/candidates.ts`
- [x] `src/parcel-tracker/carriers/carrierMap.effect.spec.ts` [spec]
- [x] `src/parcel-tracker/carriers/carrierMap.ts`
- [x] `src/parcel-tracker/effect.ts`
- [x] `src/parcel-tracker/extraction/extractDeliveries.spec.ts` [spec]
- [x] `src/parcel-tracker/extraction/extractDeliveries.ts`
- [x] `src/parcel-tracker/extraction/schema.ts`
- [x] `src/parcel-tracker/filter/keywords.spec.ts` [spec]
- [x] `src/parcel-tracker/filter/keywords.ts`
- [x] `src/parcel-tracker/index.ts`
- [x] `src/parcel-tracker/parcel/parcelApi.spec.ts` [spec]
- [x] `src/parcel-tracker/parcel/parcelApi.ts`
- [x] `src/parcel-tracker/persistence.atomic.spec.ts` [spec]
- [x] `src/parcel-tracker/persistence.spec.ts` [spec]
- [x] `src/parcel-tracker/persistence.ts`
- [x] `src/parcel-tracker/pipeline.reliability.spec.ts` [spec]
- [x] `src/parcel-tracker/pipeline.ts`

### WP03 (25 files)

- [x] `src/calendar-events/caldav/api.spec.ts` [spec]
- [x] `src/calendar-events/caldav/api.ts`
- [x] `src/calendar-events/caldav/http.spec.ts` [spec]
- [x] `src/calendar-events/caldav/http.ts`
- [x] `src/calendar-events/caldav/icloud.ts`
- [x] `src/calendar-events/caldav/ics.spec.ts` [spec]
- [x] `src/calendar-events/caldav/ics.ts`
- [x] `src/calendar-events/caldav/index.ts`
- [x] `src/calendar-events/caldav/xml.spec.ts` [spec]
- [x] `src/calendar-events/caldav/xml.ts`
- [x] `src/calendar-events/effect.ts`
- [x] `src/calendar-events/extraction/attachments.ts`
- [x] `src/calendar-events/extraction/extractEvents.ts`
- [x] `src/calendar-events/extraction/sanitize.spec.ts` [spec]
- [x] `src/calendar-events/extraction/sanitize.ts`
- [x] `src/calendar-events/extraction/schema.ts`
- [x] `src/calendar-events/filter/keywords.spec.ts` [spec]
- [x] `src/calendar-events/filter/keywords.ts`
- [x] `src/calendar-events/index.ts`
- [x] `src/calendar-events/persistence.atomic.spec.ts` [spec]
- [x] `src/calendar-events/persistence.spec.ts` [spec]
- [x] `src/calendar-events/persistence.ts`
- [x] `src/calendar-events/pipeline.reliability.spec.ts` [spec]
- [x] `src/calendar-events/pipeline.ts`
- [x] `src/mcp/tools/calendar.ts`

### WP04 (54 files)

- [x] `src/ios-controls/apns.spec.ts` [spec]
- [x] `src/ios-controls/apns.ts`
- [x] `src/ios-controls/liveSlots.spec.ts` [spec]
- [x] `src/ios-controls/liveSlots.ts`
- [x] `src/ios-controls/persistence.spec.ts` [spec]
- [x] `src/ios-controls/persistence.ts`
- [x] `src/ios-controls/routes.spec.ts` [spec]
- [x] `src/ios-controls/routes.ts`
- [x] `src/ios-controls/service.spec.ts` [spec]
- [x] `src/ios-controls/service.ts`
- [x] `src/live-check/channelsConfig.spec.ts` [spec]
- [x] `src/live-check/channelsConfig.ts`
- [x] `src/live-check/dgg.spec.ts` [spec]
- [x] `src/live-check/dgg.ts`
- [x] `src/live-check/displayOrder.spec.ts` [spec]
- [x] `src/live-check/displayOrder.ts`
- [x] `src/live-check/identityLinks.ts`
- [x] `src/live-check/metrics/index.ts`
- [x] `src/live-check/metrics/persistence.spec.ts` [spec]
- [x] `src/live-check/metrics/persistence.ts`
- [x] `src/live-check/metrics/types.ts`
- [x] `src/live-check/metrics/ViewerMetricsService.spec.ts` [spec]
- [x] `src/live-check/metrics/ViewerMetricsService.ts`
- [x] `src/live-check/metrics/windows.spec.ts` [spec]
- [x] `src/live-check/metrics/windows.ts`
- [x] `src/live-check/notificationPolicy.spec.ts` [spec]
- [x] `src/live-check/notificationPolicy.ts`
- [x] `src/live-check/outage.spec.ts` [spec]
- [x] `src/live-check/outage.ts`
- [x] `src/live-check/persistence.ts`
- [x] `src/live-check/platforms/common.spec.ts` [spec]
- [x] `src/live-check/platforms/common.ts`
- [x] `src/live-check/platforms/index.ts`
- [x] `src/live-check/platforms/kick.spec.ts` [spec]
- [x] `src/live-check/platforms/kick.ts`
- [x] `src/live-check/platforms/twitch.spec.ts` [spec]
- [x] `src/live-check/platforms/twitch.ts`
- [x] `src/live-check/platforms/youtube.spec.ts` [spec]
- [x] `src/live-check/platforms/youtube.ts`
- [x] `src/live-check/profileLinks.spec.ts` [spec]
- [x] `src/live-check/profileLinks.ts`
- [x] `src/live-check/sessions.spec.ts` [spec]
- [x] `src/live-check/sessions.ts`
- [x] `src/live-check/streamers.spec.ts` [spec]
- [x] `src/live-check/streamers.ts`
- [x] `src/live-check/task.spec.ts` [spec]
- [x] `src/live-check/task.ts`
- [x] `src/live-check/testRuntime.ts`
- [x] `src/live-check/titleDebounce.spec.ts` [spec]
- [x] `src/live-check/titleDebounce.ts`
- [x] `src/live-check/transitions.spec.ts` [spec]
- [x] `src/live-check/transitions.ts`
- [x] `src/live-check/triggerChannels.spec.ts` [spec]
- [x] `src/live-check/triggerChannels.ts`

### WP05 (25 files)

- [x] `src/live-check/intelligence/alertPolicy.spec.ts` [spec]
- [x] `src/live-check/intelligence/alertPolicy.ts`
- [x] `src/live-check/intelligence/anomaly.spec.ts` [spec]
- [x] `src/live-check/intelligence/anomaly.ts`
- [x] `src/live-check/intelligence/audio.spec.ts` [spec]
- [x] `src/live-check/intelligence/audio.ts`
- [x] `src/live-check/intelligence/classifier.ts`
- [x] `src/live-check/intelligence/localSpeech.spec.ts` [spec]
- [x] `src/live-check/intelligence/localSpeech.ts`
- [x] `src/live-check/intelligence/persistence.spec.ts` [spec]
- [x] `src/live-check/intelligence/persistence.ts`
- [x] `src/live-check/intelligence/presencePolicy.spec.ts` [spec]
- [x] `src/live-check/intelligence/presencePolicy.ts`
- [x] `src/live-check/intelligence/service.spec.ts` [spec]
- [x] `src/live-check/intelligence/service.ts`
- [x] `src/live-check/intelligence/summaryText.spec.ts` [spec]
- [x] `src/live-check/intelligence/summaryText.ts`
- [x] `src/live-check/intelligence/types.ts`
- [x] `src/live-check/intelligence/voiceEvidence.spec.ts` [spec]
- [x] `src/live-check/intelligence/voiceEvidence.ts`
- [x] `src/live-check/intelligence/voiceTargets.spec.ts` [spec]
- [x] `src/live-check/intelligence/voiceTargets.ts`
- [x] `src/tools/enroll-destiny-voice.ts`
- [x] `src/tools/livestream-intelligence-doctor.ts`
- [x] `src/types/sherpa-onnx-node.d.ts`

### WP06 (61 files)

- [x] `src/mcp/tools/press-pods.ts`
- [x] `src/press-pods/agents/cleaner.ts`
- [x] `src/press-pods/agents/metadata.ts`
- [x] `src/press-pods/agents/parsing.spec.ts` [spec]
- [x] `src/press-pods/agents/parsing.ts`
- [x] `src/press-pods/audio.ts`
- [x] `src/press-pods/costs.ts`
- [x] `src/press-pods/effect.ts`
- [x] `src/press-pods/errors.spec.ts` [spec]
- [x] `src/press-pods/errors.ts`
- [x] `src/press-pods/formatting/index.ts`
- [x] `src/press-pods/formatting/lineFiltering.spec.ts` [spec]
- [x] `src/press-pods/formatting/lineFiltering.ts`
- [x] `src/press-pods/httpError.ts`
- [x] `src/press-pods/persistence.spec.ts` [spec]
- [x] `src/press-pods/persistence.ts`
- [x] `src/press-pods/pipeline.effect.spec.ts` [spec]
- [x] `src/press-pods/pipeline.ts`
- [x] `src/press-pods/publicHttp.spec.ts` [spec]
- [x] `src/press-pods/publicHttp.ts`
- [x] `src/press-pods/retrievers/constants.ts`
- [x] `src/press-pods/retrievers/extractus.ts`
- [x] `src/press-pods/retrievers/fetch.ts`
- [x] `src/press-pods/retrievers/index.spec.ts` [spec]
- [x] `src/press-pods/retrievers/index.ts`
- [x] `src/press-pods/retrievers/jina.spec.ts` [spec]
- [x] `src/press-pods/retrievers/jina.ts`
- [x] `src/press-pods/retrievers/postlight.ts`
- [x] `src/press-pods/retrievers/readability.ts`
- [x] `src/press-pods/retrievers/removepaywall.ts`
- [x] `src/press-pods/retrievers/wayback.ts`
- [x] `src/press-pods/retrievers/x.spec.ts` [spec]
- [x] `src/press-pods/retrievers/x.ts`
- [x] `src/press-pods/routes.spec.ts` [spec]
- [x] `src/press-pods/routes.ts`
- [x] `src/press-pods/rss.ts`
- [x] `src/press-pods/rssText.spec.ts` [spec]
- [x] `src/press-pods/rssText.ts`
- [x] `src/press-pods/speech/audioChain.effect.spec.ts` [spec]
- [x] `src/press-pods/speech/audioChain.ts`
- [x] `src/press-pods/speech/coverage.spec.ts` [spec]
- [x] `src/press-pods/speech/coverage.ts`
- [x] `src/press-pods/speech/providers/elevenlabs.ts`
- [x] `src/press-pods/speech/providers/higgs.ts`
- [x] `src/press-pods/speech/providers/index.ts`
- [x] `src/press-pods/speech/providers/types.ts`
- [x] `src/press-pods/speech/stt.ts`
- [x] `src/press-pods/speech/synthesize.ts`
- [x] `src/press-pods/speech/textChunking.spec.ts` [spec]
- [x] `src/press-pods/speech/textChunking.ts`
- [x] `src/press-pods/speech/voices.ts`
- [x] `src/press-pods/storage.spec.ts` [spec]
- [x] `src/press-pods/storage.ts`
- [x] `src/press-pods/submit.spec.ts` [spec]
- [x] `src/press-pods/submit.ts`
- [x] `src/press-pods/task.ts`
- [x] `src/press-pods/types.ts`
- [x] `src/press-pods/url.spec.ts` [spec]
- [x] `src/press-pods/url.ts`
- [x] `src/tools/tts-bakeoff.ts`
- [x] `src/types/postlight-parser.d.ts`

### WP07 (58 files)

- [x] `src/alerts/castro.spec.ts` [spec]
- [x] `src/alerts/castro.ts`
- [x] `src/mcp/tools/podcasts.ts`
- [x] `src/podcast-recs/account.ts`
- [x] `src/podcast-recs/candidates.spec.ts` [spec]
- [x] `src/podcast-recs/candidates.ts`
- [x] `src/podcast-recs/castro/api.spec.ts` [spec]
- [x] `src/podcast-recs/castro/api.ts`
- [x] `src/podcast-recs/castro/auth.spec.ts` [spec]
- [x] `src/podcast-recs/castro/auth.ts`
- [x] `src/podcast-recs/castro/client.spec.ts` [spec]
- [x] `src/podcast-recs/castro/client.ts`
- [x] `src/podcast-recs/castro/inboxCleanupTask.spec.ts` [spec]
- [x] `src/podcast-recs/castro/inboxCleanupTask.ts`
- [x] `src/podcast-recs/castro/protocol.spec.ts` [spec]
- [x] `src/podcast-recs/castro/protocol.ts`
- [x] `src/podcast-recs/discovery.ts`
- [x] `src/podcast-recs/filters.spec.ts` [spec]
- [x] `src/podcast-recs/filters.ts`
- [x] `src/podcast-recs/guests.ts`
- [x] `src/podcast-recs/guestSelection.spec.ts` [spec]
- [x] `src/podcast-recs/guestSelection.ts`
- [x] `src/podcast-recs/itunes.spec.ts` [spec]
- [x] `src/podcast-recs/itunes.ts`
- [x] `src/podcast-recs/outcomes.spec.ts` [spec]
- [x] `src/podcast-recs/outcomes.ts`
- [x] `src/podcast-recs/persistence.spec.ts` [spec]
- [x] `src/podcast-recs/persistence.ts`
- [x] `src/podcast-recs/pipeline.spec.ts` [spec]
- [x] `src/podcast-recs/pipeline.ts`
- [x] `src/podcast-recs/podcastindex/auth.spec.ts` [spec]
- [x] `src/podcast-recs/podcastindex/auth.ts`
- [x] `src/podcast-recs/podcastindex/client.spec.ts` [spec]
- [x] `src/podcast-recs/podcastindex/client.ts`
- [x] `src/podcast-recs/podcastindex/types.ts`
- [x] `src/podcast-recs/reflection/evidence.ts`
- [x] `src/podcast-recs/reflection/index.ts`
- [x] `src/podcast-recs/reflection/persistence.ts`
- [x] `src/podcast-recs/reflection/reflection.spec.ts` [spec]
- [x] `src/podcast-recs/reflection/reflection.ts`
- [x] `src/podcast-recs/reflection/stats.ts`
- [x] `src/podcast-recs/reflection/task.ts`
- [x] `src/podcast-recs/reflection/types.ts`
- [x] `src/podcast-recs/rss.spec.ts` [spec]
- [x] `src/podcast-recs/rss.ts`
- [x] `src/podcast-recs/selection.ts`
- [x] `src/podcast-recs/shortlist.ts`
- [x] `src/podcast-recs/subscriptions.ts`
- [x] `src/podcast-recs/task.spec.ts` [spec]
- [x] `src/podcast-recs/task.ts`
- [x] `src/podcast-recs/taste.spec.ts` [spec]
- [x] `src/podcast-recs/taste.ts`
- [x] `src/podcast-recs/titles.ts`
- [x] `src/podcast-recs/types.spec.ts` [spec]
- [x] `src/podcast-recs/types.ts`
- [x] `src/podcast-recs/voices.spec.ts` [spec]
- [x] `src/podcast-recs/voices.ts`
- [x] `src/tools/castro-smoke.ts`

### WP08 (45 files)

- [x] `src/mcp/tools/media-shared.ts`
- [x] `src/mcp/tools/media.ts`
- [x] `src/recommendations/arr/client.ts`
- [x] `src/recommendations/arr/radarr.spec.ts` [spec]
- [x] `src/recommendations/arr/radarr.ts`
- [x] `src/recommendations/arr/sonarr.spec.ts` [spec]
- [x] `src/recommendations/arr/sonarr.ts`
- [x] `src/recommendations/candidates.enrichment.spec.ts` [spec]
- [x] `src/recommendations/candidates.spec.ts` [spec]
- [x] `src/recommendations/candidates.ts`
- [x] `src/recommendations/effect.spec.ts` [spec]
- [x] `src/recommendations/effect.ts`
- [x] `src/recommendations/filters.spec.ts` [spec]
- [x] `src/recommendations/filters.ts`
- [x] `src/recommendations/history.ts`
- [x] `src/recommendations/identity.spec.ts` [spec]
- [x] `src/recommendations/identity.ts`
- [x] `src/recommendations/mediaLibrary.spec.ts` [spec]
- [x] `src/recommendations/mediaLibrary.ts`
- [x] `src/recommendations/outcomes.spec.ts` [spec]
- [x] `src/recommendations/outcomes.ts`
- [x] `src/recommendations/persistence.spec.ts` [spec]
- [x] `src/recommendations/persistence.ts`
- [x] `src/recommendations/pipeline.spec.ts` [spec]
- [x] `src/recommendations/pipeline.ts`
- [x] `src/recommendations/plex/client.spec.ts` [spec]
- [x] `src/recommendations/plex/client.ts`
- [x] `src/recommendations/selection.ts`
- [x] `src/recommendations/shortlist.spec.ts` [spec]
- [x] `src/recommendations/shortlist.ts`
- [x] `src/recommendations/task-startup.spec.ts` [spec]
- [x] `src/recommendations/task.ts`
- [x] `src/recommendations/taste/evidence.ts`
- [x] `src/recommendations/taste/index.ts`
- [x] `src/recommendations/taste/persistence.ts`
- [x] `src/recommendations/taste/reflection.ts`
- [x] `src/recommendations/taste/stats.ts`
- [x] `src/recommendations/taste/task.ts`
- [x] `src/recommendations/taste/taste.spec.ts` [spec]
- [x] `src/recommendations/taste/types.ts`
- [x] `src/recommendations/tmdb/client.ts`
- [x] `src/recommendations/tmdb/types.spec.ts` [spec]
- [x] `src/recommendations/tmdb/types.ts`
- [x] `src/recommendations/types.ts`
- [x] `src/recommendations/watchlist.ts`

### WP09 (28 files)

- [x] `src/arr-recovery/boundaries.spec.ts` [spec]
- [x] `src/arr-recovery/client.spec.ts` [spec]
- [x] `src/arr-recovery/client.ts`
- [x] `src/arr-recovery/filesystem.spec.ts` [spec]
- [x] `src/arr-recovery/filesystem.ts`
- [x] `src/arr-recovery/llm.spec.ts` [spec]
- [x] `src/arr-recovery/llm.ts`
- [x] `src/arr-recovery/nzbget.spec.ts` [spec]
- [x] `src/arr-recovery/nzbget.ts`
- [x] `src/arr-recovery/persistence.spec.ts` [spec]
- [x] `src/arr-recovery/persistence.ts`
- [x] `src/arr-recovery/policy.spec.ts` [spec]
- [x] `src/arr-recovery/policy.ts`
- [x] `src/arr-recovery/service.spec.ts` [spec]
- [x] `src/arr-recovery/service.ts`
- [x] `src/arr-recovery/task.ts`
- [x] `src/arr-recovery/types.ts`
- [x] `src/observer-repair/agent.spec.ts` [spec]
- [x] `src/observer-repair/agent.ts`
- [x] `src/observer-repair/arr.spec.ts` [spec]
- [x] `src/observer-repair/arr.ts`
- [x] `src/observer-repair/persistence.spec.ts` [spec]
- [x] `src/observer-repair/persistence.ts`
- [x] `src/observer-repair/service.spec.ts` [spec]
- [x] `src/observer-repair/service.ts`
- [x] `src/observer-repair/task.ts`
- [x] `src/observer/client.test.ts` [spec]
- [x] `src/observer/client.ts`

### WP10 (23 files)

- [x] `src/icloud/protectedAccess.spec.ts` [spec]
- [x] `src/icloud/protectedAccess.ts`
- [x] `src/mcp/tools/reminders.spec.ts` [spec]
- [x] `src/mcp/tools/reminders.ts`
- [x] `src/reminders/apple.spec.ts` [spec]
- [x] `src/reminders/apple.ts`
- [x] `src/reminders/cloudkit.spec.ts` [spec]
- [x] `src/reminders/cloudkit.ts`
- [x] `src/reminders/cloudkitExtras.spec.ts` [spec]
- [x] `src/reminders/cloudkitExtras.ts`
- [x] `src/reminders/codec.spec.ts` [spec]
- [x] `src/reminders/codec.ts`
- [x] `src/reminders/config.ts`
- [x] `src/reminders/recurrence.spec.ts` [spec]
- [x] `src/reminders/recurrence.ts`
- [x] `src/reminders/recurringCompletion.spec.ts` [spec]
- [x] `src/reminders/recurringCompletion.ts`
- [x] `src/reminders/routes.spec.ts` [spec]
- [x] `src/reminders/routes.ts`
- [x] `src/reminders/service.spec.ts` [spec]
- [x] `src/reminders/service.ts`
- [x] `src/reminders/store.spec.ts` [spec]
- [x] `src/reminders/store.ts`

### WP11 (27 files)

- [x] `src/briefing-agent/BriefingAgentTask.ts`
- [x] `src/briefing-agent/configs.spec.ts` [spec]
- [x] `src/briefing-agent/configs.ts`
- [x] `src/briefing-agent/persistence.spec.ts` [spec]
- [x] `src/briefing-agent/persistence.ts`
- [x] `src/briefing-agent/placeholders.spec.ts` [spec]
- [x] `src/briefing-agent/placeholders.ts`
- [x] `src/mcp/tools/workspaces.ts`
- [x] `src/workspaces/actions.test.ts` [spec]
- [x] `src/workspaces/actions.ts`
- [x] `src/workspaces/definitions.test.ts` [spec]
- [x] `src/workspaces/definitions.ts`
- [x] `src/workspaces/email.test.ts` [spec]
- [x] `src/workspaces/email.ts`
- [x] `src/workspaces/emailHandler.test.ts` [spec]
- [x] `src/workspaces/engine.test.ts` [spec]
- [x] `src/workspaces/engine.ts`
- [x] `src/workspaces/errors.ts`
- [x] `src/workspaces/notifications.test.ts` [spec]
- [x] `src/workspaces/notifications.ts`
- [x] `src/workspaces/persistence.test.ts` [spec]
- [x] `src/workspaces/persistence.ts`
- [x] `src/workspaces/repository.ts`
- [x] `src/workspaces/schema.test.ts` [spec]
- [x] `src/workspaces/task.test.ts` [spec]
- [x] `src/workspaces/task.ts`
- [x] `src/workspaces/types.ts`

### WP12 (35 files)

- [x] `src/device-link/routes.spec.ts` [spec]
- [x] `src/device-link/routes.ts`
- [x] `src/device-link/service.spec.ts` [spec]
- [x] `src/device-link/service.ts`
- [x] `src/mcp/activity.spec.ts` [spec]
- [x] `src/mcp/activity.ts`
- [x] `src/mcp/activityRoutes.ts`
- [x] `src/mcp/auth.spec.ts` [spec]
- [x] `src/mcp/auth.ts`
- [x] `src/mcp/events/catalog.ts`
- [x] `src/mcp/events/claudeSessions.spec.ts` [spec]
- [x] `src/mcp/events/claudeSessions.ts`
- [x] `src/mcp/events/executorAuth.spec.ts` [spec]
- [x] `src/mcp/events/executorAuth.ts`
- [x] `src/mcp/events/persistence.ts`
- [x] `src/mcp/events/protocol.spec.ts` [spec]
- [x] `src/mcp/events/protocol.ts`
- [x] `src/mcp/events/service.spec.ts` [spec]
- [x] `src/mcp/events/service.ts`
- [x] `src/mcp/events/webhook.spec.ts` [spec]
- [x] `src/mcp/events/webhook.ts`
- [x] `src/mcp/policy.spec.ts` [spec]
- [x] `src/mcp/policy.ts`
- [x] `src/mcp/route.ts`
- [x] `src/mcp/runtime.ts`
- [x] `src/mcp/server.spec.ts` [spec]
- [x] `src/mcp/server.ts`
- [x] `src/mcp/tool.spec.ts` [spec]
- [x] `src/mcp/tool.ts`
- [x] `src/mcp/tools/claude-sessions.spec.ts` [spec]
- [x] `src/mcp/tools/claude-sessions.ts`
- [x] `src/mcp/tools/events.ts`
- [x] `src/mcp/tools/index.ts`
- [x] `src/mcp/tools/system.ts`
- [x] `src/tools/generate-mcp-policy.ts`

### WP13 (34 files)

- [x] `src/claude-resets/policy.spec.ts` [spec]
- [x] `src/claude-resets/policy.ts`
- [x] `src/claude-resets/source.spec.ts` [spec]
- [x] `src/claude-resets/source.ts`
- [x] `src/claude-resets/task.ts`
- [x] `src/codex-resets/delivery.spec.ts` [spec]
- [x] `src/codex-resets/delivery.ts`
- [x] `src/codex-resets/history.spec.ts` [spec]
- [x] `src/codex-resets/history.ts`
- [x] `src/codex-resets/policy.spec.ts` [spec]
- [x] `src/codex-resets/policy.ts`
- [x] `src/codex-resets/source.ts`
- [x] `src/codex-resets/task.ts`
- [x] `src/hister/service.spec.ts` [spec]
- [x] `src/hister/service.ts`
- [x] `src/mcp/tools/browser-history.ts`
- [x] `src/mcp/tools/personal.ts`
- [x] `src/mcp/tools/printer.ts`
- [x] `src/pet-tracker/api.spec.ts` [spec]
- [x] `src/pet-tracker/api.ts`
- [x] `src/pet-tracker/auth.ts`
- [x] `src/pet-tracker/math.ts`
- [x] `src/pet-tracker/persistence.ts`
- [x] `src/pet-tracker/seed.ts`
- [x] `src/pet-tracker/task.spec.ts` [spec]
- [x] `src/pet-tracker/task.ts`
- [x] `src/printer/service.spec.ts` [spec]
- [x] `src/printer/service.ts`
- [x] `src/reset-alerts/delivery.spec.ts` [spec]
- [x] `src/reset-alerts/delivery.ts`
- [x] `src/reset-alerts/presentation.ts`
- [x] `src/reset-alerts/source.ts`
- [x] `src/reset-alerts/task.spec.ts` [spec]
- [x] `src/reset-alerts/task.ts`

### WP14 (5 files)

- [x] `src/data-manager.spec.ts` [spec]
- [x] `src/data-manager.ts`
- [x] `src/index.ts`
- [x] `src/server.ts`
- [x] `src/tools/preview-server.ts`

### WP15 (45 files)

- [x] `frontend/src/api.spec.ts` [spec]
- [x] `frontend/src/api.ts`
- [x] `frontend/src/App.tsx`
- [x] `frontend/src/components/ActivityFeed.tsx`
- [x] `frontend/src/components/badges.tsx`
- [x] `frontend/src/components/EmailLogModal.tsx`
- [x] `frontend/src/components/ImageWithFallback.tsx`
- [x] `frontend/src/components/LiveNow.tsx`
- [x] `frontend/src/components/LogViewer.tsx`
- [x] `frontend/src/components/McpBadges.tsx`
- [x] `frontend/src/components/NavBar.tsx`
- [x] `frontend/src/components/OnDeck.tsx`
- [x] `frontend/src/components/PlatformIcon.tsx`
- [x] `frontend/src/components/SectionNav.tsx`
- [x] `frontend/src/components/ShowMore.tsx`
- [x] `frontend/src/components/StatStrip.tsx`
- [x] `frontend/src/components/StatusFilterChips.tsx`
- [x] `frontend/src/components/TaskCard.tsx`
- [x] `frontend/src/components/Toast.tsx`
- [x] `frontend/src/components/WorkspaceMarkdown.spec.tsx` [spec]
- [x] `frontend/src/components/WorkspaceMarkdown.tsx`
- [x] `frontend/src/effect.spec.ts` [spec]
- [x] `frontend/src/effect.ts`
- [x] `frontend/src/hooks/useModal.ts`
- [x] `frontend/src/hooks/useNow.ts`
- [x] `frontend/src/hooks/useRecHighlight.ts`
- [x] `frontend/src/hooks/useVisiblePoll.ts`
- [x] `frontend/src/live.tsx`
- [x] `frontend/src/main.tsx`
- [x] `frontend/src/pages/CostsPage.tsx`
- [x] `frontend/src/pages/DataPage.tsx`
- [x] `frontend/src/pages/EmailActivityPage.tsx`
- [x] `frontend/src/pages/HomePage.tsx`
- [x] `frontend/src/pages/LivestreamIntelligencePage.tsx`
- [x] `frontend/src/pages/OperationsPage.tsx`
- [x] `frontend/src/pages/StreamerPage.tsx`
- [x] `frontend/src/router.tsx`
- [x] `frontend/src/utils/claudeActivity.spec.ts` [spec]
- [x] `frontend/src/utils/claudeActivity.ts`
- [x] `frontend/src/utils/cron.ts`
- [x] `frontend/src/utils/download.ts`
- [x] `frontend/src/utils/emailLabels.ts`
- [x] `frontend/src/utils/format.ts`
- [x] `frontend/src/utils/recLabels.ts`
- [x] `frontend/src/vite-env.d.ts`

### WP16 (16 files)

- [x] `frontend/src/components/RecommendationRuns.tsx`
- [x] `frontend/src/components/TasteBrain.tsx`
- [x] `frontend/src/pages/BriefingsPage.tsx`
- [x] `frontend/src/pages/ClaudePage.tsx`
- [x] `frontend/src/pages/FeedbackPage.tsx`
- [x] `frontend/src/pages/McpPage.tsx`
- [x] `frontend/src/pages/PetsPage.tsx`
- [x] `frontend/src/pages/PodcastDetailPage.tsx`
- [x] `frontend/src/pages/PodcastsPage.tsx`
- [x] `frontend/src/pages/PodsDetailPage.tsx`
- [x] `frontend/src/pages/PodsPage.tsx`
- [x] `frontend/src/pages/RecommendationDetailPage.tsx`
- [x] `frontend/src/pages/RecommendationsPage.tsx`
- [x] `frontend/src/pages/RemindersPage.spec.tsx` [spec]
- [x] `frontend/src/pages/RemindersPage.tsx`
- [x] `frontend/src/pages/WorkspacesPage.tsx`

### WP17 (6 files)

- [x] `packages/executor-events-adapter/src/auth.ts`
- [x] `packages/executor-events-adapter/src/continuations.ts`
- [x] `packages/executor-events-adapter/src/legacy.ts`
- [x] `packages/executor-events-adapter/src/server.ts`
- [x] `packages/executor-events-adapter/test/adapter.node-test.mjs` [spec]
- [x] `packages/executor-events-adapter/test/native.node-test.mjs` [spec]

### Unassigned

none


