# Stylist

The `omni-stylist` subsystem is a fit-first personal stylist ledger: a
structured, case-based record of one person's body, garments, purchases, fit
outcomes and preferences, with a versioned runbook that tells any assistant
how to use it. It is served only through authenticated MCP tools. There is no
UI, no scheduled message, no shopping integration and no receipt scanning.

The ledger is the source of truth. Assistants read it at the start of every
session (`stylist_context_get`) instead of relying on chat memory, and write
plain-language feedback back as typed events.

## Design

- **Records** (`stylist_record_save`) have kinds `variant` (a product as sold:
  brand, model, size label and system, cut, colour, fabric, version),
  `piece` (one owned garment, linked to its variant, optionally a sizing
  reference), `measurement` (body, piece or a variant's published chart),
  `profile_snapshot`, `preference`, `prediction`, `rule` and `need`.
  Variants, pieces, preferences, rules and needs are revised with optimistic
  concurrency (`id` + `expectedRevision`) and every revision is kept.
  Measurements, profile snapshots and predictions are immutable; a correction
  is a new record with `supersedes` and a `correctionReason`.
- **Events** (`stylist_event_append`) are immutable: `acquired`, `try_on`,
  `wear`, `wash`, `alteration`, `decision`, `returned`, `retired`,
  `feedback`, `outcome`, `contradiction`, `note`. A correction is a new event
  of the same type with `supersedesEventId`. A piece's status (`owned`,
  `undecided`, `return_pending`, `returned`, `retired`) is derived from its
  lifecycle events.
- **Provenance** on every observation record (variant, piece, measurement,
  profile snapshot, preference, need) and event: `certainty` (`explicit`,
  `inferred`, `unknown`), `source`, `observedOn`, evidence ids. An event
  without provenance is stored as an explicit user statement. Predictions and
  rules carry their evidence ids and basis instead. The server records
  `recordedAt` and the ledger revision.
- **Measurements** keep the original value and unit and add `valueMm`. They
  distinguish body from finished garment, flat width from circumference,
  linear and curve lengths, endpoints, stretch and wash state, wash count and
  tolerance. A body flat width is rejected. `stylist_measurements_compare`
  compares only the same dimension, method and subject class and lists every
  other pair as incompatible without converting it.
- **Predictions** freeze a forecast before any outcome: the exact variant and
  size, per-region expectations, likely failure mode, evidence gaps,
  references and rules used, return risk, the context revision and digest the
  forecast was made from, and the runbook version. The server checks the
  claimed basis (`exact_repeat`, `known_brand_new_cut`,
  `new_brand_inference`) against the owned history and rejects a stronger
  claim. The forecast also stores the variant's revision and identity and the
  revisions of the references and rules it used, and a variant that anything
  uses cannot change its brand, model, category, size, cut, colour, version
  or SKU. Events that link a forecast (`acquired`, `try_on`, `outcome`,
  `feedback`) must be dated no earlier than the day before the freeze, must
  use a piece of the forecast's exact variant, and are rejected as
  `[post_hoc]` when that piece's fit or decision was recorded before the
  freeze (superseded reports included). A forecast with any linked event can
  no longer be superseded. `outcome` events (one per stage: `immediate`,
  `after_wear`, `after_wash`, `delayed`) score the frozen expectations per
  region (`exact`, `close`, `miss`, ...) without changing the prediction.
- **Rules** are narrow (a brand, model or category is required) and linked to
  evidence. A rule becomes `active` only when every in-scope fit case has a
  regression verdict and none is inconsistent. Later in-scope cases are listed
  as unchecked, and the rule appears in `rules.needsReview` until it is
  re-checked. Confidence is expressed through the basis tiers and qualitative
  risk, never percentages.
- **Context** (`stylist_context_get`) returns, from one ledger revision: the
  runbook and checklist, the profile and current body measurements, reference
  garments with their newest measurements and wash history, fit history
  (successes, failures, unresolved; a kept piece without a fit assessment and
  a forecast without an outcome are unresolved), brand history, current
  preferences and recent changes, rules, contradictions, warnings, due items
  and open needs. `asOfRevision` reconstructs any earlier state, evaluated on
  the date of that revision's last write so it is reproducible. A piece's
  status folds its lifecycle events in `occurredOn` order, so backfilled
  history lands where it happened.
- **Contradictions** are computed (repeated measurements disagreeing beyond
  the summed tolerances, or 15 mm for the body and 10 mm for garments when a
  tolerance is missing; conflicting fit reports with no wash or alteration
  between them; opposing preferences) or flagged with a `contradiction`
  event. A `contradiction` event with `status: "resolved"` closes a flagged
  one (`resolvesEventId`) or a computed one (listing the ids it named).

## Storage

Tables beside `blobs` in the docstore (`crates/omni-stylist/src/ledger.rs`):
`stylist_namespaces`, `stylist_records` (every revision),
`stylist_events`, `stylist_evidence` and `stylist_idempotency`. Every write is
one `BEGIN IMMEDIATE` transaction: idempotency lookup, validation against the
loaded namespace, insert, namespace revision bump and idempotency receipt.
Each committed record, event or evidence row takes the next ledger revision;
an evidence upload that matches stored bytes only records its receipt.
Reusing an `idempotencyKey` with identical input returns the stored result
(`replayed: true`); with different input it fails with
`[idempotency_key_reused]`. SQLite triggers make every ledger row
append-only and refuse any deletion in `real`; unique indexes allow one
correction per original. Stored bodies must keep decoding: new fields are
added as optional.

Evidence bytes are content-addressed files under
`/data/stylist-private/evidence/sha256/` (`.local/stylist-private` outside
Docker), directories mode 0700 and files 0600, written atomically and
re-hashed on every read. Accepted types: JPEG, PNG, WebP, HEIC/HEIF, PDF,
text, Markdown and JSON, checked against their magic bytes, at most 3,000,000
decoded bytes per upload (the `/mcp` request limit is 4 MiB). Identical bytes
in one namespace return the existing evidence id and its original metadata.
Bytes leave only through `stylist_evidence_get` (one embedded resource) and
`stylist_export` archives; nothing is served over HTTP and no public URL
exists. MCP activity records only identifiers for `stylist_*` calls
(namespace, keys, ids, record kind or event type) and only the `[code]` of an
error; bodies, notes and bytes are stored as `[private]`.

Evidence files are deleted only by name: a rejected upload or restore
removes the files it created, and a fixture purge removes the purged
namespace's own files unless another namespace or a stored backup still
names them. Nothing sweeps the directory, so a database that lost its rows
cannot empty it. The serialization of uploads and deletions is per process;
run the operator restore while no fixture purge is in progress.

## Namespaces and fixtures

Every tool requires `namespace`: `real` is the owner's ledger and
`fixture-<slug>` (lowercase letters, digits and dashes) is disposable test
data. Namespaces never share records: an id from another namespace is
`[not_found]`. Context, backups and the runbook's workflow use only `real`.

For acceptance testing, use a fresh namespace per run, for example
`fixture-acceptance-20261012a`. When finished, call `stylist_fixture_purge`
with `confirmNamespace` repeating the namespace: it writes and verifies a
pre-purge archive with evidence bytes, deletes the namespace (refusing if it
changed after the archive was taken), and removes its evidence files that
nothing else names. `stylist_restore` with that archive id brings the
namespace back. `real` can be neither purged nor restored through MCP, and
archives never cross between `real` and fixtures in either direction.

## Tools

All tools are recommended `allow` in `docs/mcp-policy.json`: they change only
Omni's private ledger and files and have no external effects or cost. The
purge is annotated destructive but limited to fixtures and reversible from
its pre-purge archive.

| Tool | Annotations | Purpose |
| --- | --- | --- |
| `stylist_context_get` | read-only | Session context and runbook (above) |
| `stylist_records_query` | read-only | Exact filters (`kinds`, `ids`, `linkedTo`, `category`, `categoryPrefix`, `brand`, `statuses`, `from`/`to`), `includeSuperseded`, `includeHistory`, `asOfRevision`, `order`, `cursor`/`limit` (max 100) over records, events or evidence |
| `stylist_measurements_compare` | read-only | Compatible deltas in mm with caveats; incompatible pairs kept apart |
| `stylist_record_save` | keyed write | Create, revise or supersede a record |
| `stylist_event_append` | keyed write | Append or correct an event |
| `stylist_evidence_store` | keyed write | Store a private photo or document |
| `stylist_evidence_get` | read-only | Metadata plus verified bytes as an embedded resource |
| `stylist_export` | write (file) | Write and verify an archive; optionally attach it (at most 16 MiB) |
| `stylist_archives_list` | read-only | Stored archives with counts and checksums |
| `stylist_restore` | write | Restore an archive into an empty fixture namespace |
| `stylist_fixture_purge` | destructive | Archive, then delete a fixture namespace |

Errors are tool errors prefixed with a code: `[validation]`, `[not_found]`,
`[revision_conflict]`, `[idempotency_key_reused]`, `[immutable]`,
`[duplicate_variant]`, `[duplicate_receipt]`, `[duplicate_outcome]`,
`[basis_mismatch]`, `[post_hoc]`, `[regression_incomplete]`,
`[incompatible_measurement]`, `[namespace]`, `[integrity]`, `[too_large]`,
`[unsupported_media]`, `[archive]`. Optional input fields are omitted, never
`null`. The full schemas are in `crates/omni-mcp-kit/golden/tools-list.json`.

## Backup and restore

An archive (`omni-stylist-export`, format version 1) is one JSON document
with every record revision, event, evidence row (with base64 bytes unless
excluded), idempotency receipt, derived relationship and runbook version,
plus SHA-256 checksums per section and a `content` checksum that excludes the
namespace name, the export time and the bytes (row timestamps are included).
Verification also checks every runbook text against its digest, that the
rows cover ledger revisions 1..N exactly once, and, for stored archives, the
file digest in the manifest. These checks catch corruption and lost rows;
they are not a signature against someone who can rewrite the file and its
checksums. A restore inserts everything into an empty namespace inside one
transaction and commits only if the restored namespace reproduces the
`content` checksum and every evidence digest matches. An archive whose
written copy fails verification is deleted. Archives hold the whole document
in memory while exporting; that is fine for a personal wardrobe but grows
with the number of photos.

- `StylistBackup` runs hourly at 23 minutes past. When the `real` ledger
  changed since the last backup it writes
  `/data/stylist-private/archives/real-r<revision>-...-backup-*.json` (evidence
  metadata and digests, no bytes) and verifies every evidence file; the 04:23
  run re-verifies an unchanged backup and its evidence files, and replaces a
  backup that fails. Retention keeps the newest 72 backups, 20 other archives
  of `real` and 50 fixture archives, so test churn never evicts an archive of
  `real`. It sends no messages; a failed run is logged and alerted like any
  failed task.
- Kopia snapshots `/home/michael/compose` (including
  `volumes/omni-notify/stylist-private`) to R2 every 20 minutes. The archives
  and evidence files are immutable once written, so the snapshot holds a
  consistent copy; the live SQLite file itself is not snapshot-consistent.
- Restore after losing the database, with the private directory recovered:

  ```bash
  docker exec omni-notify omni-notify stylist-verify \
    --archive /data/stylist-private/archives/<id>.json
  docker exec omni-notify omni-notify stylist-restore \
    --archive /data/stylist-private/archives/<id>.json --namespace real --allow-real
  ```

  `stylist-export --namespace real --out FILE` writes a portable archive with
  bytes to any path. All three accept `--db PATH` and `--private DIR`.

The SQLite connection uses `synchronous=NORMAL` (WAL), shared with the rest
of Omni: a power loss can drop the last committed writes but not corrupt the
file. The recovery point for a lost database is the newest backup, at most
about an hour old while the ledger is changing.

Tested in the suite: export, verify and restore round trips (image bytes,
relationships, superseded records), restoring `real` into a fresh database
from a backup plus the evidence directory, fixture purges on that fresh
database leaving the backup's evidence intact, and the pre-purge restore. Not
tested: recovering the directory from an actual Kopia snapshot, so this is
not yet a demonstrated disaster recovery.

## Assistant setup

Every harness reaches the tools through Omni's MCP endpoint. Executor serves
them to claude.ai, ChatGPT and other clients after a catalog refresh; Claude
Code and Codex can use Executor or the `/mcp` endpoint with the bearer token
(see `docs/mcp.md`). Give the assistant this instruction, as a project
instruction, custom instruction or `AGENTS.md` line:

> For any clothing, sizing, wardrobe or styling question, first call
> `stylist_context_get` with `namespace: "real"` and a `focus` for the
> category, brand or candidate, and follow the returned runbook and
> checklist. Treat the ledger as the only memory: record measurements,
> purchases, try-ons, decisions, washes and feedback with the stylist tools
> as they happen, never invent values, and never shop, contact retailers or
> set up alerts without my explicit approval for that action.

The runbook (`crates/omni-stylist/runbook/fit-first-v1.md`, returned by the
context) covers the session order, measurement protocols, reference
garments, the recommendation and freeze workflow, shopping-time checks,
post-purchase events, narrow learning, contradictions, onboarding and how to
translate plain-language feedback into events. Published runbook versions are
frozen by a digest test; a change ships as a new version and predictions
record the version they followed.

## Proactive scope

Context lists only genuine items: open needs whose `reviewAfter` has come,
return decisions with their deadlines, outcome follow-ups for frozen
forecasts, and post-wash checks for pieces bought on a forecast. Nothing is
pushed. Budgets on needs are preferences and never authorize a purchase.
Turning any of this into notifications would be a separate, explicitly
authorized change.
