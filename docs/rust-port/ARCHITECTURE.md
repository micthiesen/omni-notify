# omni-notify Rust port: architecture

Status: plan, 2026-10-09. Companion: `WORK_PACKAGES.md` (file ownership, per-package
acceptance tests, coverage checklist).

Goal: replace the TypeScript + Effect service (`src/`), the React SPA (`frontend/`) and,
optionally later, the executor events sidecar with idiomatic Rust and a Leptos CSR
frontend. The Rust binary runs against the **existing production
`/data/docstore.db` with no migration step that TS cannot read back**, keeps every
`AGENTS.md` invariant, and keeps every external HTTP/JSON/MCP/RSS contract
byte- or value-compatible. Rollback to the TS image on the same database must stay
possible until the TS code is deleted.

Ground rules that override convenience:

1. The SQLite `blobs` table, its key encoding and its CBOR payloads are a contract
   (section 4). Rust writes must decode in node-cbor to the same JS values TS would
   have written.
2. External contracts (section 9) are frozen by golden fixtures captured from the
   running TS service before any subsystem is ported. A Rust handler is done when it
   reproduces its fixtures.
3. Subsystem crates depend only on foundation crates. Cross-subsystem calls go
   through port traits in `omni-runtime::ports` and are wired by the binary. This is
   what lets 13 subsystem packages proceed in parallel.
4. Persisted derivations computed with JS semantics (UTF-16 lengths, `JSON.stringify`,
   `localeCompare`, `Number#toString`) go through `omni-core::js` only.

---

## 1. Workspace layout

```
Cargo.toml                 # workspace; members = ["crates/*"]; no per-crate edits needed
rust-toolchain.toml        # channel = "1.97.1", components = ["clippy","rustfmt"], profile = "minimal"
rustfmt.toml               # max_width = 100, use_field_init_shorthand = true
clippy.toml                # allow-unwrap-in-tests = true, allow-expect-in-tests = true
deny.toml                  # cargo-deny: advisories, licenses, sources (crates.io only)
.cargo/config.toml         # [alias] xtask = "run -p xtask --"
crates/
  # foundation (WP00)
  omni-core/               # clock, ids, js-compat, digests, email model + handler trait, error helpers
  omni-config/             # Config::from_env, redaction, channels path, derived defaults
  omni-store/              # SQLite actor, docstore, node-cbor-compatible codec, entities, tables
  omni-tasks/              # scheduler, task registry, run history, log capture layer, catch-up, buses
  omni-http/               # reqwest wrapper (UA), public/SSRF-guarded client, bounded bodies
  omni-alerts/             # Pushover client, alert throttle, ERROR-log alert layer + gates
  omni-mailer/             # outgoing SMTP + MIME composition (src/emails)
  omni-ai/                 # model registry, OpenAI/Anthropic/Gemini wire clients, tool loop, costs
  omni-server-kit/         # axum helpers: JSON body cap, errors, origin guard, SSE, SPA service
  omni-mcp-kit/            # MCP tool registration interface + golden tool metadata
  omni-runtime/            # AppContext, Subsystem, Ports (cross-subsystem traits), boot types
  omni-api/                # wire DTOs shared by backend and wasm frontend (serde only)
  omni-testkit/            # temp stores, test clock, fakes, no-network guard, golden helpers
  xtask/                   # golden capture, mcp-policy, compat-audit, api-diff
  # subsystems (WP01-WP13), see WORK_PACKAGES.md
  omni-imap/ omni-email/ omni-parcel/ omni-calendar/ omni-live/ omni-ios-controls/
  omni-live-intel/ omni-presspods/ omni-podcasts/ omni-media/ omni-arr/ omni-reminders/
  omni-workspaces/ omni-briefings/ omni-mcp/ omni-device-link/ omni-personal/
  # app (WP14)
  omni-notify/             # the single binary: boot, wiring, ops routes, data manager
  # frontend (WP15, WP16)
  omni-web-kit/            # api client, live data, hooks, components, charts, markdown, utils
  omni-web-pages/          # domain pages (media, podcasts, pods, workspaces, pets, reminders, ...)
  omni-web/                # trunk entry (bin), router, shell, ops pages, index.html, style/
  # WP17 (required)
  omni-events-adapter/     # Rust port of packages/executor-events-adapter (deferred)
```

Root `Cargo.toml` (complete; created once by WP00, never edited by subsystem agents
except to add a dependency line to `[workspace.dependencies]`, which is
append-only and merge-trivial):

```toml
[workspace]
resolver = "3"
members = ["crates/*"]
# default-members = ["crates/omni-notify"]  added by WP14 with the binary crate (Cargo rejects a missing default member)

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.97"
license = "MIT"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"          # omni-live-intel uses the safe sherpa-onnx wrapper only
unused_must_use = "deny"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
unwrap_used = "warn"            # allowed in tests via clippy.toml
expect_used = "warn"
dbg_macro = "warn"
print_stdout = "warn"           # logging goes through tracing; xtask opts out with #![allow]
print_stderr = "warn"
let_underscore_future = "deny"  # no silently dropped futures
large_futures = "warn"

[profile.dev]
debug = "line-tables-only"
[profile.dev.package."*"]
debug = false
[profile.release]
codegen-units = 1
lto = "thin"
strip = "debuginfo"
overflow-checks = true
[profile.wasm-release]          # used by trunk for omni-web
inherits = "release"
opt-level = "z"
lto = true
panic = "abort"
```

Every crate's `Cargo.toml` has `[lints] workspace = true` and takes all versions from
`workspace = true`. Crate names are `omni-*`; library names are `omni_*`.

Dependency direction (enforced by `xtask deps-check` in CI):

```
omni-core <- {omni-config, omni-store} <- omni-tasks <- omni-runtime <- subsystems <- omni-notify
omni-core <- omni-http <- {omni-alerts, omni-ai, omni-mailer}  -> omni-runtime
omni-api (serde only) <- omni-server-kit, subsystems, omni-web-kit
omni-mcp-kit <- omni-runtime;  omni-testkit (dev-dependency only)
subsystem crates MUST NOT depend on other subsystem crates (allowlist: omni-ios-controls -> omni-live,
omni-parcel -> omni-email, omni-calendar -> omni-email for activity/retry library functions).
```

---

## 2. Dependency table

Policy: exact `=` pins in `[workspace.dependencies]` (deadpan style), each version
released at least 14 days before it is pinned (mirrors the pnpm release-age floor;
as of 2026-10-09 that means on or before 2026-09-25). `cargo deny` enforces
crates.io-only sources. Versions marked "verify" were not confirmed against the
registry by date; WP00 confirms them before the first commit and may move to the
nearest qualifying patch.

| Crate | Pin | Features / notes |
|---|---|---|
| tokio | `=1.53.1` | `rt-multi-thread, macros, time, sync, signal, process, fs, net, io-util, test-util`(dev) |
| tokio-util | `=0.7.19` | `rt` (CancellationToken, TaskTracker), `io` |
| tokio-stream | `=0.1.19` | `sync` (BroadcastStream) |
| futures | `=0.3.34` | |
| axum | `=0.8.9` | `json, query, macros, tokio, http1, http2` |
| tower | `=0.5.3` | `util, limit, timeout` |
| tower-http | `=0.7.1` | `fs, trace, set-header, limit, timeout, compression-br, compression-gzip` |
| reqwest | `=0.13.5` | `default-features=false`; `rustls, http2, charset, json, stream, gzip, brotli, query, form, multipart, cookies` |
| rustls | `=0.23.45` | |
| tokio-rustls | `=0.26.5` | IMAP/SMTP sockets |
| rustls-platform-verifier | `=0.7.1` | must match reqwest 0.13 |
| hickory-resolver | `=0.26.3` | only if reqwest's `dns::Resolve` hook needs a resolver beyond `tokio::net::lookup_host` |
| tokio-tungstenite | `=0.30.0` | `rustls-tls-native-roots` (DGG websocket; 0.30 has no platform-verifier feature, so WP04 may pass a `Connector::Rustls` built with rustls-platform-verifier) |
| cookie_store | `=0.22.1` | `serde_json` (Reminders session, tough-cookie JSON adapter written in-house) |
| rusqlite | `=0.40.2` | `bundled, backup, limits` |
| serde | `=1.0.229` | `derive` |
| serde_json | `=1.0.151` | `float_roundtrip, preserve_order` (insertion order is needed for JS-compatible JSON) |
| serde_with | `=3.23.0` | |
| serde_path_to_error | `=0.1.20` | boot-time config and channels.json errors |
| indexmap | `=2.13.0` | `serde`; CBOR object model |
| ryu-js | `=1.0.1` | JS `Number#toString` formatting (cached locally; verify date) |
| icu_collator | `=2.2.1` | `compiled_data`; JS `localeCompare` root collation (verify date) |
| schemars | `=1.2.2` | AI structured-output schemas |
| jsonschema | `=0.58.0` | MCP tool input validation against golden schemas (`default-features=false`) |
| jiff | `=0.2.37` | `serde, tzdb-bundle-always` |
| croner | `=4.0.0` | `default-features=false, features=["jiff"]`; `Seconds::Optional`, `find_previous_occurrence` exists in 4.x |
| tracing | `=0.1.44` | |
| tracing-subscriber | `=0.3.23` | `env-filter, fmt, registry, std` |
| tracing-appender | `=0.2.5` | daily file sink for `LOGS_PATH` (or in-house writer; 14-day retention is in-house either way) |
| thiserror | `=2.0.20` | |
| anyhow | `=1.0.104` | binaries, xtask and tests only |
| rmcp | `=3.4.1` | `server, transport-streamable-http-server` (no `macros`: metadata comes from golden JSON) |
| async-imap | `=0.11.3` | `default-features=false, runtime-tokio`; spike COPYUID/APPENDUID surfacing in WP01 |
| imap-proto | `=0.16.7` | used to read response codes if async-imap hides them |
| mail-parser | `=0.11.9` | `full_encoding` |
| mail-builder | `=1.0.0` | |
| lettre | `=0.11.23` | `default-features=false; smtp-transport, tokio1, tokio1-rustls, rustls-platform-verifier, aws-lc-rs, builder` |
| html2text | `=0.17.1` | html-to-text replacement (prompt text only, drift accepted) |
| scraper | `=0.27.0` | DOM queries (linkedom replacement) |
| htmd | `=0.5.5` | turndown replacement (`fetch_url` markdown) |
| dom_smoothie | `=0.18.1` | Readability retriever |
| quick-xml | `=0.42.0` | `serialize`; CalDAV multistatus, RSS reading, Hister-free XML |
| rss | `=2.1.2` | `builders`, itunes extension; PressPods feed (golden-tested) |
| id3 | `=1.17.2` | ID3v2.3 with CHAP/CTOC/APIC |
| lofty | `=0.25.4` | audio duration (or ffprobe) |
| sherpa-onnx | `=1.13.8` | VAD, speaker embedding, nemo transducer (WP05 spike) |
| ipp | `=7.0.0` | `async-client-rustls` |
| aws-cognito-srp | `=0.2.5` | Whisker USER_SRP_AUTH (WP13 spike; fall back to in-house SRP-6a) |
| jsonwebtoken | `=11.1.0` | `default-features=false, rust_crypto, use_pem`; ES256 APNs JWT |
| serde_norway | `=0.9.42` | briefing front matter |
| sha1 / sha2 | `=0.11.0` / `=0.11.0` | |
| hmac | `=0.13.0` | |
| aes-gcm | `=0.11.1` | events seal, Reminders store (format verified in WP10) |
| subtle | `=2.6.1` | constant-time compares |
| base64 | `=0.23.1` | |
| hex | `=0.4.3` | |
| flate2 | `=1.1.10` | gzip `linesGz` (lofty 0.25.4 requires `^1.1.10`) |
| uuid | `=1.26.1` | `v4, serde` |
| url | `=2.5.8` | |
| percent-encoding | `=2.3.2` | `encodeURIComponent` set lives in omni-core::js |
| rand | `=0.10.3` | jitter, CSPRNG ids |
| regex | `=1.13.1` | |
| bytes | `=1.12.1` | |
| http | `=1.5.0` | header generics in `omni-http::RequestBuilder::header` |
| wasm-bindgen-test | `=0.3.79` | headless tests for `omni-web-kit` |
| moka | `=0.12.16` | `future`; bounded TTL caches (IMAP read caches may stay hand-rolled for byte bounds) |
| backon | `=1.6.0` | retry with exponential backoff |
| num-format | `=0.4.4` | en-US digit grouping in notification text |
| leptos | `=0.8.20` | `csr` (0.8.21 was published 2026-09-26, after the floor) |
| leptos_router | `=0.8.15` | (0.8.16 was published 2026-09-26, after the floor) |
| leptos_meta | `=0.8.6` | |
| leptos-use | `=0.19.0` | resize observer, visibility, intervals |
| reactive_stores | `=0.4.3` (the line leptos 0.8 uses) | per-streamer fine-grained snapshot updates |
| gloo-net | `=0.7.0` | `http, eventsource` |
| gloo-timers | `=0.4.0` | `futures` |
| wasm-bindgen | `=0.2.129` | equals the wasm-bindgen CLI version in the Docker web stage |
| web-sys / js-sys | `=0.3.106` | |
| wasm-bindgen-futures | `=0.4.79` | |
| console_error_panic_hook | `=0.1.7` | |
| pulldown-cmark | `=0.13.4` | `default-features=false`; tables, strikethrough, tasklists, GFM autolink post-pass |
| insta | `=1.48.0` | `json, yaml` golden snapshots |
| wiremock | `=0.6.5` | HTTP stubs |
| tempfile | `=3.27.0` | |
| proptest | `=1.11.0` | codec/key round trips |
| trunk (tool) | `0.21.14` | Docker web stage only |

Rejected: `ciborium`/`serde_cbor`/`minicbor-serde` (no first-class JS `undefined`, float width
control or tag-1 Date semantics; we write a ~600-line codec), `sqlx` (no backup API, async
transactions do not model better-sqlite3), `cron` (chrono-only), `rig-core`/`genai`
(churn; our cost/timeout/log seams are first-class in a thin client), Pi Durable (TypeScript),
`sherpa-rs` (stale), `fractional_index` (incompatible key encoding).

---

## 3. Foundation crates and public APIs

Signatures are normative: subsystem packages code against them before WP00 lands
(WP00 publishes a stub version of every foundation crate in its first commit so
parallel work compiles). `BoxFuture<'a, T>` is `futures::future::BoxFuture`.

### 3.1 `omni-core`

```rust
pub mod clock {
    pub trait Clock: Send + Sync + 'static {
        fn now_ms(&self) -> i64;                       // epoch ms (JS Date.now())
        fn now(&self) -> jiff::Timestamp { /* from now_ms */ }
    }
    pub struct SystemClock;                            // SystemTime
    pub struct TestClock { /* base epoch + tokio::time::Instant offset; follows paused time */ }
    impl TestClock { pub fn new(epoch_ms: i64) -> Arc<Self>; pub fn set(&self, ms: i64); }
    pub type SharedClock = Arc<dyn Clock>;
}
pub mod js {                                           // JS-exact semantics; golden-tested vs node
    pub fn utf16_len(s: &str) -> usize;
    pub fn utf16_slice(s: &str, start: usize, end: usize) -> Cow<'_, str>; // JS slice; a split surrogate half becomes U+FFFD (what node persists)
    pub fn number_to_string(n: f64) -> String;                      // ryu-js
    pub fn json_stringify(v: &serde_json::Value) -> String;         // JSON.stringify: insertion order, JS numbers
    pub fn json_stringify_pretty2(v: &serde_json::Value) -> String; // JSON.stringify(v, null, 2)
    pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering;  // icu root collation (V8 default)
    pub fn encode_uri_component(s: &str) -> String;
    pub fn to_iso_string(ms: i64) -> String;                        // Date#toISOString, always .mmmZ
}
pub mod digest {
    pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String;
    pub fn digest(s: &str) -> String;                  // src/utils/fingerprint.ts digest()
    pub fn fingerprint_evidence<T: serde::Serialize>(items: &[T], id: impl Fn(&T)->&str) -> Result<String, serde_json::Error>; // sha256 hex[..24]; Err if an item is not an object
    pub fn ct_eq_sha256(a: &[u8], b: &[u8]) -> bool;   // sha256 then constant-time compare
}
pub mod ids { pub fn uuid_v4() -> String; pub fn secure_id_b64url(bytes: usize) -> String; }
pub mod email {                                        // shared by WP01/02/03/11/12
    #[derive(Clone, Debug, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
    pub struct FetchedEmail {          // exact TS `FetchedEmail` shape (src/email/types.ts)
        pub id: String,                // Message-ID WITH angle brackets, or imap|<folder>|<uv>|<uid>
        pub origin: Option<EmailOrigin /* folder, uid_validity: String, uid: u32 */>,
        pub subject: String, pub from: String,
        pub to: Option<Vec<String>>, pub cc: Option<Vec<String>>, pub reply_to: Option<Vec<String>>,
        pub message_id: Option<String>, pub references: Option<Vec<String>>, pub in_reply_to: Option<String>,
        pub text_body: String, pub links: Vec<String>, pub link_metadata: Option<EmailLinkMetadata>,
        pub received_at: String,       // ISO .mmmZ
        pub attachments: Vec<EmailAttachment>,
    }
    pub struct EmailAttachment { pub blob_id: String, pub attachment_id: Option<String>, pub part_id: Option<String>,
                                 pub disposition: Option<String>, pub content_id: Option<String>, pub name: String,
                                 #[serde(rename = "type")] pub mime_type: String, pub size: u64 }
    pub struct HandlerError { pub message: String, pub transient: bool, pub source: Option<BoxError> }
    pub trait EmailHandler: Send + Sync {
        fn name(&self) -> &'static str;      // "McpEvents" | "ParcelTracker" | "CalendarEvents" | "Workspaces"
        fn handle<'a>(&'a self, emails: &'a [FetchedEmail]) -> BoxFuture<'a, Result<(), HandlerError>>;
    }
}
pub mod mail_source {                                  // implemented by omni-imap, driven by omni-email's dispatcher
    pub struct EmailPoll { pub emails: Vec<FetchedEmail>, pub commit: Box<dyn FnOnce() -> BoxFuture<'static, Result<(), PollError>> + Send> }
    pub trait MailSource: Send + Sync {
        fn poll(&self) -> BoxFuture<'_, Result<EmailPoll, PollError>>;
        fn mail_events(&self) -> tokio::sync::broadcast::Receiver<()>;   // IDLE exists, reconnect, 5-min sweep
        fn start(&self) -> BoxFuture<'_, Result<(), PollError>>; fn stop(&self) -> BoxFuture<'_, ()>;
    }
}
pub mod process {                                      // bounded subprocesses (ffmpeg, yt-dlp, pdfinfo, cupsfilter)
    pub struct Output { pub status: i32, pub stdout: Vec<u8>, pub stderr: Vec<u8> }
    pub async fn run_bounded(cmd: tokio::process::Command, stdin: Option<Vec<u8>>, stdout_cap: usize,
        stderr_cap: usize, timeout: Duration) -> Result<Output, ProcessError>;  // kill_on_drop, cap -> error
}
pub mod error { pub fn chain_message(e: &dyn std::error::Error) -> String; /* innermost cause, McpToolError rule */ }
pub mod spawn {                                        // the ONLY sanctioned way to background work
    pub fn spawn_tracked<F>(tracker: &TaskTracker, name: &'static str, fut: F) -> JoinHandle<F::Output>
        where F: Future + Send + 'static, F::Output: Send;   // instruments with Span::current()
    pub async fn must_complete<F, T>(tracker: &TaskTracker, fut: F) -> T;  // runs to completion even if caller is dropped
}
```

### 3.2 `omni-config`

```rust
#[derive(Clone, Debug)] pub struct Config { /* one typed field per variable in TS src/utils/config.ts */ }
impl Config {
    pub fn from_env(vars: &BTreeMap<String, String>) -> Result<Config, ConfigError>; // fails boot like Effect Schema
    pub fn from_process_env() -> Result<Config, ConfigError>;
    pub fn redacted_summary(&self) -> Vec<(&'static str, String)>;   // isSensitiveKey + privateConfigKeys
    pub fn db_path(&self) -> PathBuf;              // DOCKERIZED ? /data/<DB_NAME> : <DB_NAME>
    pub fn pushover_token(&self, ch: PushoverChannel) -> Option<&str>; // derived fallbacks
    pub fn email_self_address(&self) -> Option<&str>;
    pub fn model(&self, role: ModelRole) -> &str;                     // env override or code default; ModelRole and PushoverChannel live in omni-config
}
pub enum ConfigError { Invalid { key: &'static str, reason: String }, WeakToken(&'static str),
                       TokensEqual, EmailFrom(String) }
pub fn legacy_warnings(vars: &BTreeMap<String,String>) -> Vec<String>; // YT_CHANNEL_NAMES etc.
```
All variables (section 6 of the core survey, including `OMNI_DEBUG`, `FFMPEG_PATH`,
`YT_DLP_PATH`) are owned here. Token rules from `src/mcp/auth.ts` (`isStrongMcpToken`)
live here. `EMAIL_FROM` accepts only `""` or `michael@thiesen.dev`.

### 3.3 `omni-store` (docstore, CBOR, entities, tables)

```rust
pub struct Store { /* Arc: one rusqlite::Connection on a dedicated OS thread + mpsc of jobs */ }
pub struct StoreOptions { pub busy_timeout: Duration /*5s*/, pub clock: SharedClock }
impl Store {
    pub async fn open(path: &Path, opts: StoreOptions) -> Result<Store, StoreError>; // WAL, synchronous=NORMAL, schema init
    pub async fn read<R, F>(&self, f: F) -> Result<R, StoreError>
        where F: FnOnce(&Docs<'_>) -> Result<R, StoreError> + Send + 'static, R: Send + 'static;
    pub async fn write<R, E, F>(&self, f: F) -> Result<R, E>          // BEGIN IMMEDIATE ... COMMIT; rollback on Err
        where F: FnOnce(&mut Tx<'_>) -> Result<R, E> + Send + 'static, E: From<StoreError> + Send + 'static, R: Send + 'static;
    pub async fn backup_to(&self, dest: &Path) -> Result<(), StoreError>;   // rusqlite::backup (WAL-safe)
    pub async fn table<T: TableRow>(&self) -> Table<T>;                      // relational tables (pets)
}
// Docs (read view) and Tx (write view) both implement DocOps; all reads add the expiry filter.
pub trait DocOps {
    fn now_ms(&self) -> i64;
    fn get_doc(&self, pk: &str) -> Result<Option<JsValue>, StoreError>;      // CorruptRow on undecodable
    fn get_raw_row(&self, pk: &str) -> Result<Option<RawRow>, StoreError>;
    fn has_doc(&self, pk: &str) -> Result<bool, StoreError>;
    fn get_keys_by_prefix(&self, prefix: &str) -> Result<Vec<String>, StoreError>;   // LIKE ESCAPE '\' (ASCII ci)
    fn get_docs_by_prefix(&self, prefix: &str) -> Result<Vec<(String, JsValue)>, StoreError>; // skips corrupt (warn)
    fn get_docs_by_entity(&self, entity: &str) -> Result<Vec<(String, JsValue)>, StoreError>;
    fn count_by_entity(&self, entity: &str) -> Result<u64, StoreError>;
    fn get_raw_rows_by_prefix(&self, prefix: &str) -> Result<Vec<RawRow>, StoreError>; // includes expired
    fn storage_bytes_by_prefix(&self, prefix: &str) -> Result<u64, StoreError>;         // no expiry filter
    fn database_size_bytes(&self) -> Result<u64, StoreError>;
}
pub trait DocWrite: DocOps {
    fn upsert_doc(&mut self, pk: &str, data: &JsValue, meta: DocMeta) -> Result<(), StoreError>;
    fn delete_doc(&mut self, pk: &str) -> Result<bool, StoreError>;
    fn touch_doc(&mut self, pk: &str, expires_at: Option<i64>) -> Result<bool, StoreError>;
    fn cleanup_expired(&mut self, limit: u32) -> Result<u64, StoreError>;
}
pub struct DocMeta { pub entity: Option<String>, pub version: i64, pub expires_at: Option<i64>, pub updated_at: Option<i64> }
pub struct RawRow { pub pk: String, pub entity: Option<String>, pub version: i64, pub expires_at: Option<i64>,
                    pub updated_at: i64, pub data: Option<Vec<u8>> }   // INTEGER cols accept REAL
#[derive(thiserror::Error, Debug)] pub enum StoreError {
    #[error("sqlite: {0}")] Sqlite(String), #[error("corrupt row {pk}: {reason}")] CorruptRow { pk: String, reason: String },
    #[error("invalid key: {0}")] InvalidKey(String), #[error("decode {pk}: {source}")] Decode { pk: String, source: cbor::DecodeError },
    #[error("validation failed for {entity}: {reason}")] Validation { entity: &'static str, reason: String },
    #[error("store closed")] Closed }

pub mod cbor {                                     // node-cbor 10.0.12 compatible
    #[derive(Clone, Debug, PartialEq)] pub enum JsValue {
        Undefined, Null, Bool(bool), Int(i128 /* |n|<=2^64 */), Float(f64), String(String), Bytes(Vec<u8>),
        Array(Vec<JsValue>), Object(indexmap::IndexMap<String, JsValue>), Map(Vec<(JsValue, JsValue)>),
        Date(f64 /* epoch ms as JS would hold it */), Set(Vec<JsValue>), BigInt(i128), Tagged(u64, Box<JsValue>) }
    pub fn encode(v: &JsValue) -> Vec<u8>;           // node Encoder rules (section 4.3)
    pub fn decode(bytes: &[u8]) -> Result<JsValue, DecodeError>;   // decodeFirstSync rules; trailing bytes error
    pub fn to_value<T: Serialize>(t: &T) -> Result<JsValue, EncodeError>;   // serde Serializer
    pub fn from_value<T: DeserializeOwned>(v: JsValue) -> Result<T, DecodeError>; // self-describing Deserializer
    pub struct JsDate(pub i64);                    // ms; serializes as tag 1; deserializes tag0/tag1/string/number
    pub type Extra = indexmap::IndexMap<String, JsValue>;  // #[serde(flatten)] on every entity: never drop fields
}
pub mod entity {
    pub enum KeyPart { Str(String), Num(f64), Bool(bool) }
    pub trait EntityKey { fn parts(&self) -> Vec<KeyPart>; }    // impls for String, (A,B), (A,B,C), i64, f64
    pub trait Entity: Serialize + DeserializeOwned + Send + Sync + 'static {
        const NAME: &'static str;                   // e.g. "streamer-status"
        const VERSION: i64 = 0;
        const DEFAULT_TTL_MS: Option<i64> = None;   // only *-reset-delivery uses it (90 days)
        type Key: EntityKey + Send;
        fn key(&self) -> Self::Key;
        fn validate(&self) -> Result<(), String> { Ok(()) }
        fn migrate(raw: JsValue, _from: i64) -> Result<JsValue, String> { Ok(raw) }
    }
    pub fn pk<E: Entity>(key: &E::Key) -> Result<String, StoreError>;  // "$name#s<utf16len>:<v>#n<num>#b1"; InvalidKey on non-finite numbers
    pub fn prefix<E: Entity>(partial: &[KeyPart]) -> String;
    pub trait EntityOps: DocOps {                          // blanket impl for Docs and Tx
        fn get<E: Entity>(&self, key: &E::Key) -> Result<Option<E>, StoreError>;
        fn has<E: Entity>(&self, key: &E::Key) -> Result<bool, StoreError>;
        fn get_all<E: Entity>(&self) -> Result<Vec<E>, StoreError>;          // by entity column, skips corrupt
        fn get_by_prefix<E: Entity>(&self, partial: &[KeyPart]) -> Result<Vec<E>, StoreError>;
    }
    pub trait EntityWrite: DocWrite {
        fn upsert<E: Entity>(&mut self, e: &E, opts: UpsertOpts) -> Result<(), StoreError>;   // TTL rules 4.2
        fn update<E: Entity>(&mut self, key: &E::Key, f: impl FnOnce(E) -> E, opts: ModifyOpts) -> Result<Option<E>, StoreError>;
        fn patch<E: Entity>(&mut self, key: &E::Key, partial: JsObjectPatch, opts: ModifyOpts) -> Result<Option<E>, StoreError>;
        fn delete<E: Entity>(&mut self, key: &E::Key) -> Result<bool, StoreError>;
    }
    pub struct UpsertOpts { pub expires_at: Option<i64>, pub ttl_ms: Option<i64> }
    pub struct ModifyOpts { pub expires_at: Option<Option<i64>> }  // None = keep existing
    pub struct EntityDescriptor { pub name: &'static str, pub version: i64,
        pub recompute_pk: fn(&JsValue) -> Result<String, String>, pub migrate: fn(JsValue, i64) -> Result<JsValue, String> }
    impl EntityDescriptor { pub fn of<E: Entity>() -> Self; }
    pub fn migrate_all(tx: &mut Tx<'_>, entities: &[EntityDescriptor]) -> Result<MigrateReport, StoreError>; // Entity.migrateAll
}
pub mod logs_gz { pub struct LogLine { t, level, logger, msg } pub fn encode(lines: &[LogLine]) -> Result<String, StoreError>; pub fn decode(s: &str) -> Result<Vec<LogLine>, StoreError>; }
pub mod table { pub trait TableRow { const DDL: &'static [&'static str]; } pub struct Table<T> { /* typed rusqlite helpers */ } }
```

Raw-key collections that are not entities (`email-archive:*`, `email-compose:*`) use
`DocWrite::upsert_doc` with explicit `DocMeta { entity: Some("email-archive-action"), .. }`;
they are never passed to `migrate_all`.

### 3.4 `omni-tasks` (scheduler, registry, run history, log capture, buses)

```rust
pub struct CronSchedule { /* croner, Seconds::Optional, local TZ from config.TZ via jiff */ }
impl CronSchedule {
    pub fn parse(expr: &str, tz: &jiff::tz::TimeZone) -> Result<Self, InvalidScheduleError>; // rejects never-matching
    pub fn next_after(&self, t: jiff::Timestamp) -> Option<jiff::Timestamp>;
    pub fn prev_at_or_before(&self, t: jiff::Timestamp) -> Option<jiff::Timestamp>;
    pub fn next_n(&self, t: jiff::Timestamp, n: usize) -> Vec<jiff::Timestamp>;  // UI nextRuns (ISO)
    pub fn as_str(&self) -> &str;
}
pub struct TaskOptions { pub jitter: Duration, pub run_on_startup: bool }
pub enum Trigger { Schedule, Manual, Startup, Catchup }      // serialized "schedule"|"manual"|"startup"|"catchup"
pub struct RunContext { pub run_id: String, pub task_name: String, pub trigger: Trigger,
                        pub scheduled_for: Option<i64>, pub cancel: CancellationToken /* shutdown signal; advisory */ }
pub trait Task: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn display_name(&self) -> Option<&str> { None }
    fn schedule(&self) -> &CronSchedule;
    fn options(&self) -> TaskOptions;
    fn run<'a>(&'a self, cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>>;
    fn accepts_manual_input(&self) -> bool { false }
    fn run_manual<'a>(&'a self, cx: &'a RunContext, _input: serde_json::Value) -> BoxFuture<'a, Result<(), TaskError>> { self.run(cx) }
    fn last_run_summary(&self) -> Option<String> { None }
}
#[derive(thiserror::Error, Debug)] #[error("{message}")]
pub struct TaskError { pub message: String, #[source] pub source: Option<Box<dyn std::error::Error + Send + Sync>> } // stored as `error`

#[derive(Clone)] pub struct TaskRegistry { /* Arc inner */ }
impl TaskRegistry {
    pub fn new(store: Store, clock: SharedClock, bus: EventBus, tracker: TaskTracker, logs: RunLogs) -> Self;
    pub fn track(&self, task: Arc<dyn Task>) -> Result<(), DuplicateTaskError>;
    pub async fn initialize(&self) -> Result<(), StoreError>;                      // markInterruptedRuns
    pub fn run_now(&self, name: &str, input: Option<serde_json::Value>) -> Result<String, RunNowError>; // spawns tracked
    pub async fn run_now_and_wait(&self, name: &str, input: Option<serde_json::Value>) -> Result<RunOutcome, RunNowError>;
    pub async fn list(&self) -> Result<Vec<TaskInfo>, StoreError>;
    pub async fn recent_runs(&self, task: Option<&str>, limit: usize) -> Result<Vec<TaskRunData>, StoreError>;
    pub async fn run_logs(&self, run_id: &str) -> Result<Option<(TaskRunData, Vec<LogLine>, u64)>, StoreError>; // live buffer first
    pub async fn recover_missed(&self) -> Result<(), StoreError>;                  // catch-up, sequential
    pub fn names(&self) -> Vec<String>;
}
pub enum RunNowError { NotFound, AlreadyRunning, ManualInputUnsupported }   // 404 / 409 / 400
pub struct Scheduler;  impl Scheduler {
    pub fn start(registry: TaskRegistry, shutdown: CancellationToken, tracker: &TaskTracker) -> JoinHandle<()>;
}
pub mod catch_up { pub fn decide(s: &CronSchedule, now: Timestamp, evaluated_through: Option<i64>) -> CatchUpDecision; }
pub mod persistence { /* TaskRunData, TaskScheduleState, TaskRunLog entities, exact TS shapes */ }
pub mod log_capture {
    pub struct RunLogLayer; impl RunLogLayer { pub fn new(logs: RunLogs) -> Self; } // Layer; attributes events to the innermost span with field run_id
    pub fn run_span(run_id: &str, task: &str) -> tracing::Span;   // info_span!("task_run", run_id, task)
    pub fn capture_scope(logs: &RunLogs, capture_id: &str, name: &str) -> (tracing::Span, CaptureHandle); // email pipelines reuse
    impl CaptureHandle { pub fn finish(self) -> (Vec<LogLine>, u64); }
    pub const MAX_LINE_CHARS: usize = 32_768; pub const MAX_LINES: usize = 20_000;
}
#[derive(Clone)] pub struct EventBus { /* broadcast::Sender<TaskRunEvent>, broadcast::Sender<RunLogEvent>, broadcast::Sender<AppEvent> */ }
impl EventBus { pub fn task_runs(&self) -> broadcast::Receiver<TaskRunEvent>; pub fn run_logs(&self) -> broadcast::Receiver<RunLogEvent>;
                pub fn app(&self) -> broadcast::Receiver<AppEvent>; pub fn emit_app(&self, e: AppEvent); }
pub enum AppEvent { DataDeleted, StreamersChanged }   // triggers dashboard snapshot rebroadcast
pub fn current_run() -> Option<RunAttribution>;      // run_id + task name from span scope (cost attribution)
```
Semantics carried verbatim from mitools `Scheduler.ts` and `src/task-runs/*`: one loop per
task; next fire is the first cron match after the previous run completes; jitter is a
uniform sleep in `[0, jitter)` before each run; a run, once started, is never cancelled
(it runs inside `must_complete`); errors log `Error running task "<name>"` at ERROR
(target `Scheduler`), which flows into the alert layer; per-task semaphore shared by all
triggers; `recordRunStartAndMarkSchedule` in one transaction including the 50-run prune;
run ids `"<task>:<uuidv4>"`.

### 3.5 `omni-http`

```rust
pub const USER_AGENT: &str = "OpenAI File Downloader, XaiImageApiFetch/1.0";
#[derive(Clone)] pub struct HttpClient { inner: reqwest::Client }
impl HttpClient {
    pub fn new(cfg: HttpConfig) -> Result<Self, HttpError>;          // UA, rustls, no default timeout
    pub fn with_overrides(self, o: HttpOverrides) -> Self;             // test base-URL rewrites
    pub fn request(&self, m: Method, url: Url) -> RequestBuilder;      // returns omni wrapper
    pub fn raw(&self) -> &reqwest::Client;                             // for streaming (CalDAV, webhooks)
}
pub struct RequestBuilder { /* wraps reqwest::RequestBuilder */ }
impl RequestBuilder {
    pub fn timeout(self, d: Duration) -> Self; pub fn header(self, k, v) -> Self; pub fn json<T: Serialize>(self, t: &T) -> Self;
    pub fn form(self, f: &[(&str, &str)]) -> Self; pub fn redirect(self, p: RedirectRule) -> Self;   // Error | None | Follow(n)
    pub async fn send_bounded(self, max_bytes: usize) -> Result<BoundedResponse, HttpError>;     // Content-Length precheck + stream count
    pub async fn json_bounded<T: DeserializeOwned>(self, max_bytes: usize) -> Result<T, HttpError>; // non-2xx -> Status
}
pub struct BoundedResponse { pub status: StatusCode, pub headers: HeaderMap, pub body: Bytes, pub final_url: Url }
#[derive(thiserror::Error, Debug)] pub enum HttpError { Timeout, Network(String), Status { status: u16, body: String /* <=4 KiB */ },
    TooLarge { limit: usize }, Decode(String), Blocked(String), InvalidUrl(String) }
impl HttpError { pub fn is_transient(&self) -> bool; }   // Timeout | Network | 5xx | 408 | 429
pub mod public {                                          // src/effect/publicHttp.ts
    pub fn is_public_address(ip: IpAddr) -> bool;
    pub fn assert_public_http_url_syntax(u: &str) -> Result<Url, HttpError>;
    pub struct PublicHttpClient;   // reqwest with dns::Resolve filtering every lookup, redirects followed manually with revalidation
    impl PublicHttpClient { pub fn new(base: &HttpClient) -> Self; pub fn request(&self, m: Method, url: Url) -> RequestBuilder; }
}
pub enum SideEffectMode { Live, Record }   // Record: compat shadow runs; every mutating adapter checks it
```

### 3.6 `omni-alerts`

```rust
#[derive(Clone, Copy)] pub enum PushoverChannel { General, Live, Briefing, Workspace, Calendar, Recs, Podcast, PressPods }
pub struct PushoverMessage { pub message: String, pub title: Option<String>, pub url: Option<String>,
    pub url_title: Option<String>, pub priority: Option<i8>, pub sound: Option<String>, pub timestamp: Option<i64> }
pub enum PushOutcome { Sent, SkippedNoToken, Disabled /* no user */ , Recorded /* SideEffectMode::Record */ }
#[derive(thiserror::Error, Debug)] #[error("pushover {status:?}: {body}")]
pub struct PushoverError { pub status: Option<u16>, pub body: String }
impl PushoverError { pub fn is_definite_rejection(&self) -> bool; }   // 4xx
#[derive(Clone)] pub struct Pushover { /* HttpClient, user, Config tokens */ }
impl Pushover {
    pub async fn send(&self, ch: PushoverChannel, m: PushoverMessage) -> Result<PushOutcome, PushoverError>; // 10 s timeout
    pub async fn send_with_token(&self, token: &str, m: PushoverMessage) -> Result<PushOutcome, PushoverError>; // channels.json tokens
}
pub mod throttle { pub struct AlertThrottle; /* in-memory 15m,30m,1h,3h; 6h reset; 500 keys LRU; repeat suffix */ }
pub trait AlertGate: Send + Sync { fn applies(&self, title: &str) -> bool;
    fn should_notify<'a>(&'a self, title: &'a str) -> BoxFuture<'a, bool>; }   // castro gate (WP07)
pub struct AlertLayer;   // tracing Layer: every ERROR event -> gates -> throttle -> Pushover General
impl AlertLayer { pub fn new(p: Pushover, gates: Arc<RwLock<Vec<Arc<dyn AlertGate>>>>, clock: SharedClock) -> (Self, AlertWorker); } // AlertWorker::run drains an mpsc
```

### 3.7 `omni-mailer` (outgoing; ports `src/emails/*`)

```rust
pub const OUTGOING_EMAIL_FROM: &str = "michael@thiesen.dev";
pub enum SmtpConfig { Explicit { host, port, user, pass, implicit_tls: bool }, ICloud { user, app_password } }
pub fn resolve_compose_config(c: &Config) -> Option<SmtpConfig>;  // full explicit, else none-set -> iCloud, partial -> None
pub struct PreparedEmail { pub from: String, pub date_iso: String, pub wire_b64: String, pub content_b64: String, pub message_id: String }
pub fn prepare_composed_email(input: &ComposeInput, message_id: &str, date_ms: i64) -> Result<PreparedEmail, MailError>; // wire: no Bcc
pub struct SendReport { pub accepted: Vec<String>, pub rejected: Vec<String> }
#[derive(Clone)] pub struct Mailer;
impl Mailer {
    pub fn new(cfg: SmtpConfig, mode: SideEffectMode) -> Self;
    pub async fn send_notification(&self, to: &str, subject: &str, html: &str, text: &str) -> Result<(), MailError>;
    pub async fn send_raw(&self, recipients: &[String], raw: &[u8]) -> Result<SendReport, MailError>; // verifies single From == fixed, no Sender/Resent-From
}
```

### 3.8 `omni-ai` (model registry, clients, tool loop, costs)

```rust
pub enum Provider { OpenAi, Anthropic, Google }
pub struct ModelId { pub provider: Provider, pub model: String }   // parse("openai:gpt-6-luna")
pub enum ModelRole { Briefing, Workspace, Extraction, CalendarExtraction, Triage, RecsShortlist, RecsSelection,
    TasteReflection, PodcastTasteReflection, LivestreamIntelligence, PressPodsMetadata, PressPodsCleaning,
    ObserverRepair, ArrRecovery }    // defaults: gpt-6-luna except Workspace/CalendarExtraction/RecsSelection/PressPodsCleaning = gpt-6-sol
pub struct Message { pub role: Role, pub content: Vec<ContentPart> }
pub struct GenerateRequest { pub system: Option<String>, pub messages: Vec<Message>, pub tools: Vec<ToolSpec>,
    pub output: Option<OutputSpec /* name + strict JSON schema */>, pub max_output_tokens: Option<u32>,
    pub reasoning_effort: Option<ReasoningEffort>, pub max_retries: u32 /* default 2 */, pub timeout: Duration /* 5 min */ }
pub struct Usage { pub input_tokens: u64, pub input_no_cache_tokens: u64, pub cache_read_tokens: u64,
    pub cache_write_tokens: u64, pub output_tokens: u64, pub reasoning_tokens: u64 }
pub struct GenerateResponse { pub text: String, pub tool_calls: Vec<ToolCall>, pub usage: Usage, pub finish: FinishReason }
pub trait LanguageModel: Send + Sync { fn id(&self) -> &ModelId;
    fn generate<'a>(&'a self, req: &'a GenerateRequest) -> BoxFuture<'a, Result<GenerateResponse, AiError>>; }
#[derive(Clone)] pub struct Ai { /* HttpClient, keys, CostRecorder, fake override for tests */ }
impl Ai {
    pub fn model(&self, id: &ModelId) -> Result<Arc<dyn LanguageModel>, AiError>;
    pub fn model_for(&self, cfg: &Config, role: ModelRole) -> Result<Arc<dyn LanguageModel>, AiError>;
    pub async fn generate_object<T: DeserializeOwned + schemars::JsonSchema>(&self, m: &dyn LanguageModel,
        req: GenerateRequest, cost: CostTag) -> Result<(T, Usage), AiError>;          // strict schema, records cost per step
    pub async fn generate_text(&self, m: &dyn LanguageModel, req: GenerateRequest, cost: CostTag) -> Result<(String, Usage), AiError>;
    pub async fn run_tool_loop(&self, m: &dyn LanguageModel, req: GenerateRequest, tools: &ToolSet,
        max_steps: u32, cost: CostTag, on_step: &mut (dyn FnMut(&StepRecord) + Send)) -> Result<LoopResult, AiError>;
}
pub struct CostTag { pub feature: Option<&'static str> /* None => current_cost_feature() */, pub operation: &'static str }
pub trait AiTool: Send + Sync { fn spec(&self) -> ToolSpec;
    fn call<'a>(&'a self, args: serde_json::Value) -> BoxFuture<'a, Result<serde_json::Value, String>>; }
pub mod schema { pub fn strict_schema<T: schemars::JsonSchema>() -> serde_json::Value; } // additionalProperties:false, all required, nullable
pub mod tools { pub struct WebSearch; /* Tavily, 15 s, 1 MiB, cost 0.8c */ pub struct FetchUrl; /* public client + htmd */ }
pub mod costs {                                   // src/costs/* + src/ai/cost.ts
    pub struct CostEventData { /* exact TS shape */ }  impl Entity for CostEventData { const NAME: &str = "cost-event"; .. }
    pub struct CostRecorder; impl CostRecorder { pub async fn record(&self, e: NewCostEvent); } // never fails caller; logs
    pub fn llm_cost_cents(model: &str, u: &Usage) -> Option<f64>;  pub const TTS_CHARACTER_CENTS: &[(&str, f64)];
    pub fn current_cost_feature(fallback: &'static str) -> &'static str;   // run task-name substring mapping
    pub async fn import_historical_costs(store: &Store) -> Result<(), StoreError>;  // "historical-v1", once
    pub fn summarize(events: &[CostEventData], days: omni_api::costs::CostRange, now: i64, tz: &TimeZone) -> omni_api::costs::CostsResponse;
}
#[derive(thiserror::Error, Debug)] pub enum AiError { Timeout, Http(HttpError), Provider { status: u16, message: String },
    Schema(String), Refused(String), StepLimit, MissingKey(Provider) }
```
Wire modules: `openai_responses` (`POST /v1/responses`, `text.format={type:"json_schema",strict:true}`,
`reasoning.effort`, function tools, usage), `anthropic_messages`, `gemini` (only when a
`google:` override is configured). Fake: `omni_testkit::FakeModel` scripted by request
fingerprint.

### 3.9 `omni-server-kit`

```rust
pub struct JsonBody<T>(pub T);  // extractor: 64 KiB cap (Content-Length precheck + streamed), 413 {"error":"Request body too large"},
                                 // invalid JSON -> treated as null -> T decode fails -> 400 {"error": <msg>}
pub struct JsonBodyLimit<const N: usize, T>(pub T);  // device-link result 512 KiB
pub fn api_error(status: StatusCode, msg: impl Into<String>) -> Response;   // {"error": msg}
pub fn same_origin_mutation_guard() -> impl Layer<Route, Service: ..> + Clone;  // /api/* rule + nosniff (from_fn middleware)
pub fn bearer_digest_eq(header: Option<&HeaderValue>, expected: &str) -> bool;   // ^Bearer (\S+)$ ci, sha256+ct
pub mod sse {
    pub fn event(name: &str, id: u64, data: &str) -> axum::response::sse::Event;
    pub fn with_ping(stream: impl Stream<Item = Event>, every: Duration, clock: SharedClock) -> impl Stream<Item = Result<Event, Infallible>>; // fixed cadence; data: epoch ms
    pub const HEADERS: [(&str, &str); 1] = [("x-accel-buffering", "no")];
}
pub fn spa_service(dist: PathBuf) -> axum::Router;  // ServeDir.precompressed_br/gzip, index.html fallback, cache headers
pub fn reminders_page_headers() -> impl Layer<Route, Service: ..> + Clone; // Cache-Control, Referrer-Policy, CSP incl 'wasm-unsafe-eval'
```

### 3.10 `omni-mcp-kit` (tool registration interface)

```rust
pub enum ExecutorPolicy { Allow, RequireApproval, Block }
pub struct ToolMeta { pub name: String, pub title: String, pub description: String,
    pub input_schema: Arc<serde_json::Map<String, Value>>, pub output_schema: Arc<serde_json::Map<String, Value>>,
    pub annotations: Annotations, pub policy: ToolPolicy }       // loaded from golden JSON, never hand-written
pub fn golden_meta(name: &str) -> Result<&'static ToolMeta, ToolMetaError>; // crates/omni-mcp-kit/golden/tools-list.json + docs/mcp-policy.json
pub struct ToolContext { pub call_id: String, pub cancel: CancellationToken }
pub enum ToolOutput { Structured(serde_json::Map<String, Value>), Custom { structured: serde_json::Map<String, Value>, content: Vec<Content> } }
#[derive(thiserror::Error, Debug)] #[error("{message}")] pub struct ToolError { pub phase: ToolPhase, pub message: String }
pub trait ToolHandler: Send + Sync {
    fn call<'a>(&'a self, input: Value, cx: ToolContext) -> BoxFuture<'a, Result<ToolOutput, ToolError>>; }
pub struct McpTool { pub meta: &'static ToolMeta, pub handler: Arc<dyn ToolHandler> }
pub fn typed_tool<I, O, F, Fut>(name: &str, f: F) -> Result<McpTool, ToolMetaError> // validate input vs golden schema (jsonschema),
    where I: DeserializeOwned, O: Serialize, F: Fn(I, ToolContext) -> Fut + Send + Sync + 'static,  // serde decode with explicit defaults,
          Fut: Future<Output = Result<O, ToolError>> + Send;                                         // validate output vs schema
pub fn paginate<T: Serialize>(items: Vec<T>, cursor: usize, limit: usize) -> Page<T>;   // {items, nextCursor|null, total}
pub fn truncate_utf16(s: &str, max: usize) -> (String, bool);
```
Each subsystem returns `Vec<McpTool>`; the binary orders them as in
`src/mcp/tools/index.ts` and fails boot if the set differs from the golden tool list.

### 3.11 `omni-runtime` (composition)

```rust
#[derive(Clone)] pub struct AppContext {
    pub config: Arc<Config>, pub store: Store, pub clock: SharedClock, pub http: HttpClient,
    pub public_http: PublicHttpClient, pub pushover: Pushover, pub mailer: Option<Mailer>, pub ai: Ai,
    pub costs: CostRecorder, pub tasks: TaskRegistry, pub bus: EventBus, pub shutdown: CancellationToken,
    pub tracker: TaskTracker, pub ports: Ports, pub side_effects: SideEffectMode, pub paths: AppPaths }
pub struct AppPaths { pub data_dir: PathBuf, pub assets_dir: PathBuf, pub web_dist: PathBuf, pub reminders_private: PathBuf,
                      pub presspods_audio: PathBuf }
pub struct Subsystem {
    pub name: &'static str,
    pub router: axum::Router,                      // state already applied
    pub tasks: Vec<Arc<dyn Task>>,
    pub mcp_tools: Vec<McpTool>,
    pub entities: Vec<EntityDescriptor>,           // for migrate_all + compat audit
    pub managed_entities: Vec<ManagedEntity>,      // data manager (slug, label, description, warning, can_delete, after_delete)
    pub boot_steps: Vec<BootStep>,                 // run in declared phase order before the server starts
    pub services: Vec<BackgroundService>,          // long-lived loops: dispatcher, delivery worker, IMAP actor
    pub email_handlers: Vec<Arc<dyn EmailHandler>>,
    pub alert_gates: Vec<Arc<dyn AlertGate>>,
}
pub struct BootStep { pub phase: BootPhase, pub name: &'static str,
    pub run: Box<dyn FnOnce(AppContext) -> BoxFuture<'static, Result<(), BootError>> + Send> }
pub enum BootPhase { Migrate, Reconcile, Services, AfterServer }
pub struct BackgroundService { pub name: &'static str,
    pub start: Box<dyn Fn(AppContext) -> BoxFuture<'static, ()> + Send + Sync>, pub retry: Option<RetryPolicy> } // Fn: restarted on exit; email: 30s..300s
pub mod ports {       // cross-subsystem traits; set once during wiring (OnceLock), read via accessors
    pub struct Ports { /* OnceLock<Arc<dyn ...>> per port */ }
    pub trait EmailReader { fetch_by_id(id, fresh) ; search(q) ; health() }          // impl WP01, used by WP02, WP11
    pub trait ArchiveEcho { is_archive_action_message(message_id, origin) -> bool } // impl WP01, used by WP12
    pub trait EmailRetryHandlers { handler(pipeline) -> Option<Arc<dyn EmailHandler>> } // impl WP14 wiring, used by WP02 retry/reprocess
    pub trait CalendarWriter { create_event(uid, &CalendarEventInput) -> Created|AlreadyExists ; status() }  // impl WP03, used by WP11
    pub trait LiveDirectory { streamers(); statuses(); display(); details(id) }      // impl WP04, used by WP12, WP14
    pub trait LiveIntelligence { after_tick(); on_transition(); details(id, limit); diagnostics(); record_feedback() } // impl WP05, used by WP04, WP12
    pub trait OnDeckSource { on_deck() -> Vec<OnDeckItem> }                          // impl WP08, used by WP14
    pub trait BriefingsReader { histories() }                                        // impl WP11, used by WP12
    pub trait ClaudeSessionNotifier { note_turn_started(session) }                   // impl WP12 events, used by WP12 tools (same WP)
    pub trait PodcastAccount { ... }                                                 // impl + use WP07 (kept local)
}
```

### 3.12 `omni-api` (shared DTOs)

`serde` + `serde_json` only; compiles for `wasm32-unknown-unknown`. `lib.rs` (created by
WP00) declares every module; each module file is owned by one package:

| Module | Owner | Contents |
|---|---|---|
| `common` | WP00 | `ApiErrorBody{error}`, `Paginated`, `Ms` alias, path builders + `encode_uri_component` |
| `tasks`, `runs`, `costs` | WP00 | `TaskInfo`, `Run`, `RunLogLine`, `RunLogsResponse`, SSE `init/line/done`, `CostsResponse` |
| `snapshot`, `data` | WP14 | `Snapshot{tasks,streamers,runs,onDeck}`, data-manager DTOs |
| `streamers`, `ios` | WP04 | `StreamerView` (manual serde on boolean `live`), metrics, sessions, trigger channels, `LiveSlotState`, registrations, diagnostics |
| `intelligence` | WP05 | details, feedback |
| `email` | WP02 | activity, logs, rules, feedback |
| `presspods` | WP06 | episodes, jobs, details |
| `podcasts` | WP07 | recommendation, taste profile |
| `media` | WP08 | recommendation (with `links`), taste profile, `OnDeckItem` |
| `workspaces`, `briefings` | WP11 | all workspace/briefing payloads |
| `mcp_activity`, `claude` | WP12 | MCP activity, Claude activity/sessions/transcript |
| `reminders` | WP10 | `PublicStatus` |
| `pets` | WP13 | pets |

Serde rules: `rename_all = "camelCase"`; TS `T|null` → `Option<T>` always serialized;
TS `field?: T` → `#[serde(default, skip_serializing_if = "Option::is_none")]`; string
unions → enums with explicit `rename`; never `deny_unknown_fields`. Every DTO has a
round-trip test against the golden fixture of the endpoint that returns it.

### 3.13 `omni-testkit`

```rust
pub struct TestStore { pub store: Store, _dir: TempDir }    // file-backed WAL db in tempdir
impl TestStore { pub async fn new(clock: SharedClock) -> Self; pub async fn from_fixture(path: &Path) -> Self; } // copy of golden db
pub fn test_clock(epoch_ms: i64) -> Arc<TestClock>;          // pair with #[tokio::test(start_paused = true)]
pub struct TestApp { pub ctx: AppContext, pub pushes: RecordedPushes, pub mails: RecordedMails, pub ai: FakeModels }
impl TestApp { pub async fn new() -> Self; pub fn router(&self, s: &Subsystem) -> axum::Router; pub async fn get_json(..); pub async fn post_json(..) }
pub struct FakeModels;  impl FakeModels { pub fn script(&self, role: ModelRole, responses: Vec<GenerateResponse>); }
pub fn no_network() -> HttpClient;        // resolver refuses all hosts; tests must use wiremock base URLs
pub async fn mock_server() -> wiremock::MockServer;
pub fn capture_logs() -> LogCapture;       // tracing subscriber scoped to the test
pub fn golden(path: &str) -> serde_json::Value;   // crates/<crate>/tests/golden/<path>
pub mod node_cbor_fixtures { pub fn load(name: &str) -> (Vec<u8>, serde_json::Value /* JS view */); } // encode cases of omni-store tests/golden/cbor.json
```
Tests never reach the network: `TestApp` builds `HttpClient` with `no_network()`, Pushover
and SMTP are recorders, `SideEffectMode::Record` is the default.

### 3.14 F0 reconciliation (WP00 stub commit)

The stub commit makes every section-3 signature compile. Where a signature above was
inconsistent or incomplete, the code is normative and the change is listed here.

- Placement: `ModelRole` and `PushoverChannel` are defined in `omni-config` (which `Config`
  needs and which `omni-ai`/`omni-alerts` depend on) and re-exported from `omni-ai` and
  `omni-alerts`. `LogLevel` is `omni_core::LogLevel`; `LogLine` (`TaskRunLogLine`) is
  `omni_store::LogLine`, re-exported by `omni-tasks`.
- Every domain error enum carries a temporary `Unimplemented(&'static str)` variant (structs
  such as `PushoverError` say "not implemented" in their message); it goes away as each body
  lands. Types whose constructor is stubbed (`Store`, `Docs`, `Tx`, `CronSchedule`) hold an
  uninhabited field, so their remaining bodies are statically unreachable instead of faking
  results.
- Added constructors the composition needs: `StoreOptions::new(clock)`, `EventBus::new(cap)`
  (+ `emit_task_run`, `emit_run_log`), `RunLogs::new(bus, clock)`, `Pushover::new(http, &Config,
  mode)` (+ `recorded()` in Record mode), `Ai::new(http, &Config, CostRecorder)` +
  `Ai::with_override(Arc<dyn ModelOverride>)` (the `FakeModels` seam), `CostRecorder::new(store,
  clock)`, `WebSearch::new`, `FetchUrl::new`, `Subsystem::named`.
- `HttpConfig { connect_timeout, offline }` (`offline` installs a refuse-all DNS resolver for
  `omni_testkit::no_network`); the client disables automatic redirects and applies
  `RedirectRule` per request (default `Error`). `RequestBuilder` also has `query`,
  `bearer_auth`, `basic_auth`, `body`, `url`.
- `Config` has `pub` fields named after the env keys (snake_case) plus `omni_debug`,
  `ffmpeg_path`, `yt_dlp_path`; `Debug` prints the redacted summary. `SMTP_PORT` and
  `FRONTEND_PORT` stay `f64` (TS coerces `""` to 0); `resolve_compose_config` returns `None`
  for a port outside 1..=65535. `db_path` concatenates `/data/` + `DB_NAME` exactly like TS, so
  the production `DB_NAME=/data/docstore.db` resolves to `/data//data/docstore.db` in both.
- `omni-store`: `pk`/`prefix` and `logs_gz::encode` return `Result`; `StoreError::Encode`
  added; `JsObjectPatch = IndexMap<String, JsValue>`; `MigrateReport { migrated,
  collisions_skipped, failed }`; `prefix` ends with `#` after each given part. `JsValue`/`JsDate`
  serde protocol: reserved names `$omni::cbor::{Undefined,Date,Set,BigInt,Tagged}`
  (see `omni_store::cbor` docs) that the F1 serializer/deserializer must honor.
- `omni-tasks`: entities are `persistence::{TaskRunData, TaskScheduleState, TaskRunLog}`;
  `TaskRunEvent { kind, task_name }`, `RunLogEvent::{Line, End}`; `RunNowError` gains
  `Store`; `catch_up::decide` returns `None` without a persisted cursor; `RunOutcome { run }`.
- `omni-alerts`: `throttle::{AlertThrottle::new/admit, alert_key, format_elapsed}`; the alert
  worker logs its own delivery failures at WARN so alerts never feed themselves.
- `omni-mcp-kit`: `Content = serde_json::Map` (an MCP content block served verbatim);
  `ToolPolicy { side_effects, cost, recommended_policy }`; `Annotations` has the four hints;
  `Page<T> { items, next_cursor, total }`.
- `omni-runtime`: `BackgroundService::start` is `Fn` (a service with a `RetryPolicy` is
  restarted); `ManagedEntity { slug, label, description, warning, entity, primary_key,
  can_delete, after_delete }`. Ports have concrete signatures in `omni_runtime::ports`;
  payloads that are DTOs owned by another package's `omni-api` module (streamers,
  intelligence, on-deck, briefings) cross as `serde_json::Value` of that DTO. `PodcastAccount`
  stays inside WP07 and is not a runtime port.
- `omni-testkit`: `TestApp::{get_json, post_json}` take the `&Router` to call;
  `capture_logs()` returns a `LogCapture` with `events()`.
- `omni-api`: WP00 modules define `common::{ApiErrorBody, Paginated, Ms, paths}`,
  `tasks::{TaskInfo, TasksResponse, RunNowResponse}`, `runs::{Run, RunLogLine,
  RunLogsResponse, RunsResponse, RunLogStreamFrame}`, `costs::{CostsResponse, CostRange, ...}`.

F1 (`omni-core`, `omni-store`, `omni-config`) additions and changes, all additive except
where noted:

- `omni-core`: `js::string_to_number` (JS `Number(string)`; used by config decoding and
  CBOR tag-1 coercion). `process::run_bounded` is real; `ProcessError::Unimplemented` is gone.
- `omni-store::cbor`: `JsValue::Simple(u8)` (node `Simple`, re-encodes byte-identically);
  `SIMPLE_TOKEN`; `ValueDeserializer` and `undefined_as_none` are public; `JsValue` helpers
  `as_str/as_f64/get/as_object/as_object_mut/is_nullish`; `MAX_DEPTH = 128` nesting limit
  (node has none; production depth is far below). The omni deserializer's `deserialize_any`
  presents JS-only values as single-entry token maps (BigInt as its decimal string) so
  `#[serde(flatten)] extra` stays lossless; the serializer folds those maps back. Decoder
  divergences, none of which node's encoder writes: tag 0 parses RFC 3339 or `YYYY-MM-DD`
  only; tags 32/35 keep their text (node normalizes URL/RegExp); BigInts over 127 bits stay
  `Tagged(2|3, Bytes)` (still byte-identical on re-encode); tags above 2^31-1 are errors
  (as in node). The `*Error::Unimplemented` variants are gone.
- `omni-store` docstore: `DocOps::count_by_prefix`, `DocWrite::{delete_docs_by_prefix,
  delete_docs_by_entity, clear}`, `Docs::connection()` / `Tx::connection()` (raw SQL inside
  the job), `RawRow::decode()`, `Store::{path, clock}`, `like_prefix`. "Now" is read on the
  calling task when the job is queued (paused test clocks apply). A panic inside a job rolls
  the transaction back and resumes on the caller. `storage_bytes_by_prefix` escapes the
  prefix (TS `data-manager.ts` uses an unescaped `LIKE '$name#%'`; identical for every
  entity name in use).
- `omni-store` entities: `EntityOps::count`, `EntityWrite::{delete_all, touch}`,
  `ModifyOpts::ttl_ms`. Change: `update`/`patch` reject a result whose primary key differs
  (`StoreError::Validation`) instead of silently re-asserting the key fields as TS does;
  typed collection reads also skip rows the typed model rejects (warned). `migrate_all` runs
  in the caller's single transaction; a row whose payload the typed model rejects counts as
  `failed` and is left untouched.
- `omni-store::table`: `TableRow { NAME, COLUMNS, DDL, to_values, from_row }`,
  `Table::{insert, upsert, query, all, clear}`, `SqlValue`/`Row` re-exports, and
  `table::pets::{PetRow, WeightHistoryRow}` with mitools' exact DDL. Change:
  `Store::table` returns `Result<Table<T>, StoreError>` (its DDL can fail).
- `omni-store::logs_gz`: lone-surrogate `\udXXX` escapes (a line truncated inside a
  surrogate pair) decode as U+FFFD instead of failing the whole log.
- `omni-config`: `Config::{ffmpeg_bin, yt_dlp_bin}` (`|| default`); a missing or empty
  `OMNI_MCP_TOKEN` where one is required is `ConfigError::Invalid { key: "OMNI_MCP_TOKEN" }`;
  positive-integer fields typed `u32` reject values of 2^32 or more (TS would accept them).

F2 (`omni-tasks`, `omni-http`, `omni-alerts`, `omni-mailer`, `omni-server-kit`) additions and
changes:

- `omni-tasks`: `RunNowError` drops `Unimplemented` and gains `RunFailed { run_id, message }`
  (`run_now_and_wait` fails only after the failed run is persisted, as TS) and `Shutdown`.
  Dropping a `run_now_and_wait` caller never cancels the run, so the TS registry case
  "interrupts the native task Effect" is dropped (reason in the test header).
  `TaskRegistry::{shutdown, is_running}` added; `Scheduler::start` calls `shutdown()` on
  cancel, abandoning manual runs still queued while started runs finish. `list` reports
  three next fires (`getNextRuns(3)`).
- Cron and DST: croner fires a spurious run at the end of a spring-forward gap, so a time
  inside the gap is shifted forward by the gap length, as Effect does. Effect also fires
  the first daily match after spring-forward one hour late; that is fixed, not copied. The
  parity table (`tests/golden/cron-parity.{mjs,json}`) checks every task-map schedule over
  2026 for daily/weekly schedules and over both DST transition windows for sub-hourly
  ones (a full year of 15-20 s schedules is ~1.5M fires); the 8 affected pairs are pinned
  exactly.
- Log and alert capture keep only events at DEBUG or above whose target has no `::` or
  starts with `omni_` (hyper/rustls internals stay out of run logs and alerts). Run-log
  lines render `msg key=value` instead of TS's JSON-serialized arguments. WP14's console
  layer must filter per layer (`with_filter`), never globally, or the run-log layer loses
  DEBUG lines.
- Scheduler tests run on real time (~6 s): the store's OS thread defeats tokio's
  paused-time auto-advance.
- `omni-http`: `HttpError::Unimplemented` removed; a redirect under `RedirectRule::Error`
  is `Status { 3xx }`; too many redirects is `Blocked`; the public client follows up to
  10 redirects by default. Added `allow_loopback_for_tests()`, `filter_dns_answers()` and
  async `assert_public_http_url()`. `is_transient` keeps 408 (TS retries only 429 and 5xx).
  Dropped publicHttp cases (abort signal, late headers, rejected stream cancellation)
  and the sse `awaitSseWriter` case have reasons in their test files.
- `omni-alerts`: `Pushover::with_credentials`; the alert worker prefixes titles with
  `Error: ` like mitools `logHook`.
- `omni-mailer`: `resolve_compose_values` (pure core of `resolve_compose_config`),
  `send_composed`, `verify_sender_identity`, `plaintext_for_tests`,
  `MailError::InvalidRecipient`. lettre stops at the first rejected recipient, so any
  partial rejection is an error (never success).
- `omni-server-kit`: `ApiError` / `ApiResult`, `app_router` (merges routers, applies the
  same-origin guard and panic catching, SPA fallback or JSON 404 without a built
  frontend), `sse::enqueue_initial_snapshot_frame`.

F3 (`omni-ai`, `omni-mcp-kit`, `omni-testkit`, `xtask`) additions and changes:

- `omni-ai` types: `GenerateResponse` gains `reasoning` and `provider_items` and implements
  `Default`; `ContentPart::ProviderItems`; `ToolResult.name`; `LanguageModel::role()`
  (default method); `LoopResult.stopped_at_step_limit` and `LoopResult::object()`;
  `Ai::{with_keys, generate}`. `AiError::Unimplemented` removed.
- `omni-ai` behavior: reaching the step limit returns `Ok` (AI SDK parity); `StepLimit` comes
  only from `LoopResult::object()`. Costs are recorded by the `Ai` helpers; calling
  `LanguageModel::generate` directly records nothing. `import_historical_costs` returns
  `Result<u64, StoreError>`. `runId` is written as JS `undefined` outside a run, like TS rows.
- Tools: `WebSearch::new` takes a `PublicHttpClient` and a `CostRecorder`; `WebSearch` and
  `FetchUrl` have `with_http` for tests (the guarded client refuses loopback mocks).
- `FakeModels` is scripted by role or model id (plus scripted provider failures), not by
  request fingerprint.
- `omni-mcp-kit`: `ToolMetaError::InvalidSchema` replaces `Unimplemented`; adds `raw_tool`,
  `SchemaValidator`, and the `golden` / `registry` modules (`ToolRegistry`,
  `RegistryServer` over rmcp). The offline golden (`cargo xtask mcp-golden`) is built from
  `src/mcp` with inert services, so its handshake has no `capabilities.events` (WP12 adds
  it); `capture-golden` writes `tools-list.live.json` rather than overwriting it.
- `xtask compat-audit` is row level (counts, expiry, NULL data, CBOR decode, byte-identical
  re-encode, legacy NULL-entity rows). Typed per-entity checks stay with the WP14
  `omni-notify compat-audit` subcommand.

WP00 integration:

- No `Unimplemented` variant or stub body remains in any foundation crate.
- `omni_core::error::{IntegrationError, PersistenceError}` port `src/effect/errors.ts`
  (`"<operation> failed: <cause>"`); `tests/interop.rs` ports `interop.spec.ts`.
- rmcp is pinned with `default-features = false` (`base64, server,
  transport-streamable-http-server`), so `rmcp-macros` is not built.
- One CBOR vector generator: `cargo xtask golden-cbor [--check]` runs
  `crates/omni-store/scripts/golden-cbor.mjs` (mitools `encodeDoc`/`decodeDoc`) and writes
  `crates/omni-store/tests/golden/cbor.json`; `golden-check` includes the check.
  `omni_testkit::node_cbor_fixtures::load(name)` returns an `encode` case's bytes and
  described JS value from that file.
- `TestApp::new` boots `Config::from_env(&test_app_env())` (fake Pushover and SMTP
  credentials) so pushes and mails reach the recorders.
- Production-copy checks are `#[ignore]` tests reading `OMNI_PROD_COPY` (a copy; each test
  copies it again into a tempdir): `omni-store` `prod_roundtrip`, `omni-ai` `prod_copy`,
  `omni-tasks` `prod_copy`. Rust-written `task-run` rows are value-equal to TS rows but not
  byte-identical: TS stores `scheduledFor`/`error`/`summary` as explicit `undefined` and
  appends `finishedAt` last; Rust omits absent fields and keeps struct order. node reads
  both as the same values.

---

## 4. Data compatibility contract

### 4.1 SQLite
- One connection on a dedicated thread (`Store`). `PRAGMA journal_mode=WAL`,
  `synchronous=NORMAL`, `busy_timeout=5000`. Writes use `BEGIN IMMEDIATE`; reads run on
  the same connection, so every read observes the latest commit (better-sqlite3 parity).
- Schema init copies `initializeSchema` exactly: `CREATE TABLE IF NOT EXISTS blobs(...)`,
  `PRAGMA table_info` + the four `ALTER TABLE ... ADD COLUMN` tolerating duplicates, both
  indexes. Columns are always addressed by name (production order is
  `pk, data, entity, version, expires_at, updated_at`).
- Read filter `(expires_at IS NULL OR expires_at > :now)` everywhere except
  `get_raw_rows_by_prefix`. Prefix match is `pk LIKE :p ESCAPE '\'` with `%`, `_`, `\`
  escaped (ASCII case-insensitive, deliberately preserved).
- INTEGER columns are read with `get::<_, f64>` fallback, so REAL values decode.
- Relational tables `pets`, `pet_weight_history` keep their DDL; `INSERT OR REPLACE` /
  `INSERT OR IGNORE` semantics.
- New in Rust: a `StoreMaintenance` task (hourly, not registered in the task registry UI
  by default) calling `cleanup_expired(1000)`. Reads already hide expired rows, so this
  is invisible to TS.

### 4.2 Keys and entity semantics
`$<name>#` + parts joined by `#`: `s<utf16 len>:<value>`, `n<Number#toString>`,
`b1`/`b0`. `upsert` writes `{entity, version, expires_at: opt.expires_at ?? now+ttl ??
now+DEFAULT_TTL ?? NULL, updated_at: now}`; `update`/`patch` keep `expires_at` unless
given, re-spread key fields, validate, and run read-modify-write in one transaction.
`migrate_all` at boot reproduces `Entity.migrateAll` (including expired rows, collision
skip, `updated_at || now`). WP00 first runs
`SELECT count(*) FROM blobs WHERE entity IS NULL AND pk LIKE '$%'` on the prod copy; if
zero, `migrate_all` still runs (cheap) but is not on the critical path.

### 4.3 CBOR codec (node-cbor 10.0.12 parity)
Encoder: numbers integral with |n| <= 2^53-1 → shortest major 0/1; otherwise f32 when
`(x as f32) as f64 == x`, else f64; NaN `f97e00`, ±Inf `f97c00/f9fc00`, -0 `f98000`;
strings definite UTF-8; maps definite with insertion order (struct field order = TS
object literal order, then `extra`); arrays definite; `JsDate` → tag 1 over `ms/1000`
(integer when divisible, else f32/f64 rule); `Set` → tag 258; bytes → major 2;
Optional fields follow the same serde rules as `omni-api`: TS `field?: T` is `Option<T>` with
`skip_serializing_if = "Option::is_none"` (omitted; TS reads absent and `undefined` identically),
while TS `T | null` is a plain `Option<T>` that encodes `None` as CBOR null (e.g. `costCents`).
Decoder: accepts f16/f32/f64, tags 0/1 (Date), 2/3 (BigInt), 258 (Set), 64-87 typed
arrays (as bytes), simple 23 → `Undefined` (deserializes as `None`/default), non-text
map keys → `JsValue::Map`, rejects trailing bytes, empty and NULL input (corrupt row).
Date decode truncates `v*1000` toward zero like `new Date`. Integers above 2^53 decode to
`Int`, typed fields reject them.
Every entity struct carries `#[serde(flatten)] extra: Extra` so read-modify-write never
drops fields; numeric fields accept CBOR int or float (`JsValue` deserializer coerces
integral floats to `i64` and ints to `f64`).

### 4.4 Other persisted formats
- `linesGz` = base64(gzip(`JSON.stringify(TaskRunLogLine[])`)); legacy raw `lines`.
- Reminders private store `0x01|iv12|tag16|ct`, AAD `omni-reminders:v1:<identity>`,
  tough-cookie `CookieJar.serializeSync()` JSON inside (WP10 golden).
- MCP events seal: base64(`nonce12|tag16|ct`), key `HMAC(token,"omni-mcp-events-storage-v1")`.
- PressPods audio files, `.chunks/<workId>/<key>.wav` checkpoints, render signature.
- JS-JSON-derived identities (all via `omni_core::js::json_stringify` + sha256): observer
  `issueRevision`, `fingerprintEvidence`, taste evidence ids, iOS slot hash, printer
  duplicate key, briefing `deliveryId`, compose `fingerprint`, attachment stable id,
  events `subscriptionId`/`eventId`/canonical args, workspace action `payload`.

### 4.5 Verification against a copy of production

1. **Capture** (WP00, before porting): on boris, `docker exec omni-notify node -e` is not
   needed; stop nothing. Run `sqlite3 /home/michael/compose/volumes/omni-notify/docstore.db
   ".backup /tmp/omni-copy.db"` (WAL-safe online backup), copy to the dev machine as
   `~/omni-port/prod-YYYYMMDD.db`, `chmod 600`. Never commit it.
2. **Node fixture generators** (`cargo xtask golden-cbor`, which runs
   `node crates/omni-store/scripts/golden-cbor.mjs`): encode edge numbers, Dates,
   undefined, Sets, Maps, tags and invalid inputs with mitools `encodeDoc`/`decodeDoc`;
   writes `crates/omni-store/tests/golden/cbor.json`. Entity-shaped documents are covered
   by the production-copy round trips instead.
3. **`omni-notify compat-audit --db <copy> [--rewrite-to <tmp.db>]`** (WP14 subcommand,
   uses every subsystem's `EntityDescriptor` + typed decoders):
   - per entity: row count, decode failures (typed), `recompute_pk(data) == pk` mismatches,
     unknown entity names, legacy NULL-entity `$` rows, expired counts;
   - round trip: `decode → typed → encode → decode` must equal the original `JsValue`
     modulo `Undefined` fields removed; report byte-identical percentage (informational);
   - with `--rewrite-to`: writes every row re-encoded by Rust into a fresh DB, then
     `xtask node-readback <tmp.db>` decodes all rows with node-cbor and diffs against the
     original JS values. Zero diffs is the cutover gate for rollback safety.
4. **API diff**: run TS (`node dist/index.js --server-only`, `DB_NAME=<copy A>`) and Rust
   (`omni-notify --server-only --side-effects=record`, `DB_NAME=<copy B>`); `xtask api-diff`
   GETs every read route in WORK_PACKAGES.md section "route map" with the ids found in the
   DB and diffs JSON values (key presence and nulls included), plus `/pods/rss` XML,
   `tools/list` and `docs/mcp-policy.json`.
5. **Shadow run**: Rust with full scheduler, `--side-effects=record` (Pushover, SMTP, IMAP
   writes, CalDAV, Arr/Castro/Plex/Overseerr mutations, APNs, webhooks, printer, Parcel are
   recorded, not sent; reads go live) on a fresh copy for 24 h. Diff its writes against the
   TS production DB delta for the same window (same entities touched, no extra
   notifications in the recording beyond what TS sent).
6. **Cutover**: stop `omni-notify`, `.backup` to `docstore.pre-rust.db`, deploy Rust image,
   watch `docker logs`. **Rollback**: redeploy the last TS image; same DB.

---

## 5. Conventions replacing "Effect-first TypeScript"

### Errors
- One `thiserror` enum per domain module (`ImapError`, `CaldavError { transient: bool, status: Option<u16> }`,
  `ParcelError`, ...), mirroring the TS `Data.TaggedError` set. Expected failures are
  `Result::Err`; panics are bugs (equivalent of defects) and are caught only at task and
  request boundaries (`catch_unwind` via tokio `JoinError::is_panic`, logged at ERROR).
- `anyhow` only in `omni-notify` main, `xtask` and tests.
- Wrap foreign errors at the leaf (`map_err`), never stringly-propagate. `is_transient()`
  methods replace `transient` flags where retry decisions depend on them.
- HTTP handlers return `Result<Json<T>, ApiError>` where `ApiError: IntoResponse`
  produces `{"error": "..."}` with the TS status code. Unhandled errors map to 500
  `text/plain` "Internal Server Error" (Hono parity).

### Async and concurrency
- tokio multi-thread runtime. No fire-and-forget: every spawned task goes through
  `omni_core::spawn::spawn_tracked` on the app `TaskTracker`; shutdown = cancel token, then
  `tracker.close(); tracker.wait()` with a 30 s bound.
- Fan-out: `JoinSet` or `futures::stream::iter(..).buffer_unordered(n)` with the TS
  concurrency limits (e.g. live check 6 / 4, retrievers 7, Castro 4 + 8 rps).
- Work that must not be interrupted (task runs, durable reservation + external call +
  record sequences) runs inside `must_complete`, so a dropped HTTP request or shutdown
  cannot split it. Everything else is cancel-safe by construction (no state between
  awaits that a drop would corrupt).
- Locks: `tokio::sync::Mutex`/`Semaphore` for async critical sections (IMAP operation
  permit, archive workflow, outbox `stateLock`/`drainLock`, Reminders account lock);
  never hold a lock across webhook, Executor, or Pushover I/O (AGENTS.md).
- Time: all "now" goes through `ctx.clock`; all sleeping through `tokio::time` so
  `start_paused` tests control it. Scheduling uses `omni-tasks`; no raw intervals except
  in services documented in WORK_PACKAGES.md (IMAP sweep, SSE ping, device-link hold).
- Caches: `moka` or explicit bounded structs; in-memory state that TS resets on restart
  stays in memory (title debouncer, pending peaks, outage episode, anomaly samples,
  alert cooldowns, Kick token, nonce cache, throttle, voice evidence).
- Idempotency: notifications, queue operations, external writes and retries reserve
  durably before the side effect and record after; uncertain outcomes are never retried.

### Logging
- `tracing` everywhere. Logger names (shown in the UI log viewer) come from the event
  `target`; each module declares `const LOG: &str = "LiveCheck";` and uses
  `info!(target: LOG, ...)` matching the TS `NamedLogger` names.
- Subscriber stack (built in `omni-notify`): `EnvFilter(LOG_LEVEL)` → console fmt layer
  (`HH:mm:ss.mmm [LEVEL] <target> msg`, info/debug stdout, warn/error stderr) → optional
  daily file layer under `LOGS_PATH` (`omni-notify-YYYY-MM-DD.log`, 14-day retention) →
  `RunLogLayer` (unfiltered by LOG_LEVEL, run attribution via span) → `AlertLayer`
  (ERROR only).
- Never log secrets, prompt text for Claude sessions, or email bodies at info.

### Testing
- `cargo test --workspace`. Pure decisions are plain `#[test]`; async uses
  `#[tokio::test(start_paused = true)]` + `TestClock`. Typed failures are asserted with
  `assert!(matches!(res, Err(ParcelError::Rejected { status: 422, .. })))`.
- Every TS `*.spec.ts` / `*.test.ts` maps to a Rust test module named after it
  (`tests/<spec_stem>.rs` or `#[cfg(test)] mod <stem>_spec`); WORK_PACKAGES.md lists them.
  A spec case may be dropped only with a written reason in the test file header.
- Golden fixtures (insta JSON snapshots and raw files) for every external contract.
- No network in tests; wiremock for HTTP, in-process fakes for IMAP/SMTP/CalDAV.
- Real-integration scripts are `#[ignore]` tests run manually with `dotenvx run --`.

### Lints and gates
`cargo fmt --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
`cargo test --workspace --locked`, `cargo deny check`, `trunk build --release` for
`omni-web`, `xtask golden-check` (tools-list, mcp-policy, api fixtures unchanged).
These replace `pnpm check:write && pnpm test && pnpm build` in AGENTS.md at cutover.

---

## 6. Web server composition

`omni-notify` builds one `axum::Router`:

```
Router::new()
  .merge(ops::router(ctx))                   // /api/health, tasks, task-runs, snapshot, events, costs, data/*
  .merge(subsystem routers ...)              // each already nested at its absolute paths
  .nest_service("/mcp", mcp_service)         // WP12: bearer layer -> events pre-router -> rmcp
  .route_layer(...)                          // none global except below
  .layer(same_origin_mutation_guard) on /api/* (path-filtered middleware)
  .fallback_service(spa_service(web_dist))   // GET only; unknown /api/* GET -> JSON 404 (documented change)
```
Order-sensitive rules preserved: iOS HMAC middleware on `/api/ios-controls/*`, Reminders
guards on `/api/reminders/*`, exact `/reminders` headers (CSP now includes
`'wasm-unsafe-eval'`), `/pods/episodes|rss|audio/:file|logo.jpeg` are concrete routes and
win over the SPA's `/pods` and `/pods/:id`. Cache: `/assets/*` immutable, `text/html`
`no-cache`. Shutdown: stop accepting, close MCP, drop SSE streams, wait tracker.

---

## 7. Frontend

- Leptos 0.8 CSR, built by trunk into `crates/omni-web/dist` (`OMNI_WEB_DIST` override).
  No SSR, no server functions: the REST contracts are the only API.
- `omni-web-kit`: `api.rs` (gloo-net; GET retry on network/502/503/504, backoff 500 ms →
  8 s cap, 7 retries; POST/DELETE never retry; `ApiClientError::{Api{status,message},
  Network, Decode}`), `live.rs` (`LiveData` context: snapshot store, `ConnectionState`,
  `run_task`), hooks (`use_now`, `use_visible_poll`, `use_modal`, `use_deep_link_target`),
  components (NavBar, SectionNav, TaskCard, LogViewer, LiveNow, OnDeck, ActivityFeed,
  StatStrip, Toast, badges, McpBadges, ImageWithFallback, ShowMore, StatusFilterChips,
  PlatformIcon, EmailLogModal, WorkspaceMarkdown), charts (`BarChart`, `LineChart` with
  brush; hand-written SVG), utils (format, cron, claudeActivity, emailLabels, recLabels,
  download).
- **Live updates**: one `EventSource("/api/events")` per tab. On `snapshot` frames,
  decode `omni_api::snapshot::Snapshot` and replace the store (streamers keyed by id via
  `reactive_stores`, lists rendered with `<For key=..>`); `ping` ignored. Parallel poll of
  `/api/snapshot` immediately and every 10 s, skipped while SSE is live. On `error`:
  `connection = Polling`; if `readyState == CLOSED`, reconnect after 5 s. Run logs:
  `EventSource("/api/task-runs/:id/logs/stream")` with `init`/`line`/`done`. MCP and
  Claude pages use visibility-aware 10 s polling. The server side (`omni-notify::ops::sse`)
  keeps the 150 ms debounce, identical-payload skip, fresh initial snapshot under a
  semaphore, 25 s `ping`, and global monotonically increasing ids.
- Styling: `frontend/src/index.css` moves verbatim to `crates/omni-web/style/index.css`;
  class names unchanged. `index.html` keeps theme color and favicon; trunk
  `inject_scripts = false` with an external hashed boot script under `/assets/` so the
  `/reminders` CSP needs only `'wasm-unsafe-eval'`.
- Router: leptos_router with routes in TS precedence (`/streamers/:id/intelligence` before
  `/streamers/:id`, `/workspaces/:w/:s` before `/workspaces/:w`), `/recommendations` →
  `/media` redirect, trailing-slash normalization, scroll-to-top and `#main-content` focus
  on navigation, `"<Section> · Omni Notify"` titles.
- Size: `wasm-release` profile + wasm-opt `z` + brotli precompression served by
  `ServeDir::precompressed_br()`.

---

## 8. Deployment and CI

Dockerfile (replaces the current one at cutover; same image name, same compose service,
same `/data` volume, same port `3000` mapped to `8080` in compose):

```
FROM rust:1.97.1-bookworm AS server        # cargo build --release --locked -p omni-notify (cargo-chef layer cache)
FROM rust:1.97.1-bookworm AS web           # rustup target add wasm32-unknown-unknown; cargo install trunk@0.21.14
                                           # wasm-bindgen-cli@0.2.129; trunk build --release crates/omni-web/index.html
FROM debian:bookworm-slim AS livestream-assets   # unchanged: yt-dlp (sha-pinned), silero, campplus, parakeet (sha-pinned)
FROM node:24.19.0-slim AS runtime          # keeps Node for yt-dlp --js-runtimes node; Debian bookworm base
RUN apt-get install cups ffmpeg ghostscript poppler-utils printer-driver-brlaser ca-certificates \
 && ppdc ... brl2360d.ppd -> /usr/share/omni-printing/brother-hll2370dw.ppd      # unchanged
COPY --from=server /target/release/omni-notify /usr/local/bin/omni-notify
COPY --from=server <sherpa-onnx shared libs if dynamic> /usr/local/lib/  (+ ldconfig)  # WP05 decides static vs dynamic
COPY --from=web /crates/omni-web/dist ./web
COPY assets ./assets                        # press-pods/{denoise.rnnn,intro.mp3,logo.jpeg}
COPY --from=livestream-assets /models ./assets/livestream-intelligence/models
COPY --from=livestream-assets /yt-dlp /usr/local/bin/yt-dlp
COPY docs/licenses ./licenses               # plus cargo-about generated Rust licenses
ENV DOCKERIZED=true DB_NAME=/data/docstore.db OMNI_WEB_DIST=/app/web
USER node; EXPOSE 3000
HEALTHCHECK CMD ["omni-notify", "healthcheck"]   # GET /api/health on FRONTEND_PORT
CMD ["omni-notify"]
```
Runtime invariants checked by `omni-notify doctor` at image build (`RUN omni-notify doctor
--image`): `ffmpeg -filters` contains `arnndn`, `firequalizer`; `ffmpeg -encoders` contains
`libmp3lame`; `assets/press-pods/denoise.rnnn` exists; `/usr/lib/cups/filter/rastertobrlaser`,
`/usr/sbin/cupsfilter`, `pdfinfo`, the PPD and the three sherpa model paths exist; `yt-dlp
--version` runs.

CI (`.github/workflows/ci.yml`, Rust job added alongside Node until cutover, Node job removed
after): toolchain from `rust-toolchain.toml`; `Swatinem/rust-cache`; fmt, clippy `-D
warnings`, `cargo test --workspace --locked`, `cargo deny check`, `trunk build --release`,
`wasm-bindgen-test` headless for `omni-web-kit`, `xtask golden-check`, executor adapter
`node --test` suite pointed at the Rust binary (`OMNI_BASE_URL`), multi-arch
(`linux/amd64,linux/arm64`) docker build; push to GHCR on `main` as today (boris deploy
timer unchanged). `executor-events-adapter.yml` unchanged.

---

## 9. External contracts frozen by golden fixtures (captured in WP00)

`xtask capture-golden --base http://omni.boris --mcp-token-env OMNI_MCP_TOKEN` (run once
against production, read-only GETs and MCP `tools/list` only) writes:
- `crates/omni-api/tests/golden/http/<route>.json` for every GET route (ids substituted),
- `crates/omni-mcp-kit/golden/tools-list.json` (both protocol eras) + copy of
  `docs/mcp-policy.json`,
- `crates/omni-presspods/tests/golden/rss.xml` (and ETag),
- SSE transcripts for `/api/events` (first frame) and one finished run's log stream,
- iOS payloads from `ios/OmniLive/StaticTests/StaticContractTests.swift`.
Executor adapter contract: `packages/executor-events-adapter/test/*.mjs` must pass against
Rust Omni.

---

## 10. Open decisions and risks

1. rmcp 3.4 support for the `events/*` custom methods and `capabilities.events` in modern
   `server/discover`; fallback is an axum pre-router (WP12 spike, day 1).
2. Top-level `oneOf` input schemas (7 tools) through rmcp `Tool` (golden JSON is served
   verbatim; spike verifies rmcp does not reject it).
3. async-imap must surface COPYUID/APPENDUID response codes and allow a single
   `UID EXPUNGE <uid>`; fallback is driving `imap-proto` on the raw stream (WP01 spike).
4. sherpa-onnx crate linking in the Debian runtime and embedding parity with existing
   voiceprints (score >= 0.62 on known clips) (WP05 spike).
5. mailparser parity (`partId`, attachment set/order, bracketed Message-IDs) needs a golden
   corpus of real messages (WP01/02); html-to-text output drift accepted for prompts only.
6. `@postlight/parser` and `@extractus/article-extractor` have no Rust equivalents; the port
   keeps the retriever names but replaces implementations (`postlight` → `dom_smoothie`
   with Mercury-style cleanup, `extractus` → `scraper` heuristics). Persisted
   `retrieverName` values remain valid strings.
7. `localeCompare` parity via ICU4X must be golden-tested against stored fingerprints;
   if it diverges, the no-op guard misfires once (one extra reflection run), not data loss.
8. lettre aborts on first RCPT rejection (stricter than nodemailer; still `sent=false`).
9. Known TS defects deliberately fixed, not mirrored: CalendarEvents activity rows never
   written (missing `yield*`), Twitch GQL username interpolation. Doc drift recorded:
   castro cleanup is 6-hourly; "swept at boot" comment; cursor-deletion warning text.
10. Unknown `/api/*` GETs return JSON 404 instead of the SPA (no client depends on it).
