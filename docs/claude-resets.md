# Claude Code reset alerts

`ClaudeResets` runs at startup and every minute (`0 * * * * *`), using the same
`PUSHOVER_USER` and `PUSHOVER_TOKEN` as `CodexResets`. It appears in Omni's task
list and supports manual runs. No new credentials are needed.

## Source and meaning

The source is Reset Radar's public structured catalog:
<https://resetradar.com/data/events.json>. It supplies event IDs, event dates,
classification, affected surfaces, source links, and descriptive text.

Only confirmed historical `counter-reset` events affecting Claude Code are
eligible. Predictions, future events, incidents, ordinary rolling windows,
limit increases, and promotion deadlines do not trigger alerts. A tracker label
is not account verification: community reports can also be marked confirmed.
Notifications say a reset was reported and retain the source description.

The catalog does not supply a reliable structured banked/immediate distinction
or redemption expiry. Never infer those from an elapsed deadline or claim that
an account refilled. Banked resets require redemption in Claude Settings > Usage;
check the account for eligibility and expiration. The source button links to the
report's primary source.

`claude-resets.com/api/resets` was also evaluated, but on October 7, 2026 the
host and Boris DNS returned `0.0.0.0` for that domain. Reset Radar was reachable
from Boris. No DNS configuration was changed.

## Delivery and operations

The 48-hour event lookback applies on every run, including startup, so enabling
the task does not replay old announcements or still-redeemable banked grants.
Catalog edit dates are not a polling heartbeat: a quiet feed remains usable.
Each run logs fetch time, catalog update date, event count, and newest event,
at INFO only when they change and at debug otherwise.
HTTP or schema errors fail the task visibly rather than masquerading as no news.

Both providers use `omni-personal`'s `reset_alerts` module for bounded HTTP/JSON/schema reads,
one-minute task execution, source snapshot logging, summaries, and durable
Pushover delivery. Reads have a 20-second timeout and a 2 MiB cap. Claude's
90-day reservations use `claude-reset-delivery`; Codex retains its existing
`codex-reset-delivery` namespace and keys. Event/source aliases suppress
cosmetic revisions and duplicate reports. Ambiguous delivery is never retried
automatically; a definite Pushover 4xx rejection releases the reservation.

Pushing main deploys the normal container. Verify the image revision and the
`ClaudeResets` startup/scheduled run under `/api/tasks`. A second run should
handle existing current signals without sending them again. Do not erase
reservations to test deduplication. A run with zero current signals verifies
source access and selection, not end-to-end device delivery.
