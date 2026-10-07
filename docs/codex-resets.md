# Codex reset alerts

`CodexResets` runs at startup and every minute (`0 * * * * *`). It uses
the existing `PUSHOVER_USER` and `PUSHOVER_TOKEN`; both must be configured for
registration. The task appears in Omni's task list and supports manual runs.

Bounded source reads, scheduling, and durable delivery are shared with
`ClaudeResets` through `src/reset-alerts/`. Provider evidence rules remain
separate; the existing Codex entity namespace and delivery keys are unchanged.
See [Claude Code reset alerts](claude-resets.md).

## Sources and meaning

The primary source is Reset Beacon's public JSON API:

- `https://resetbeacon.com/api/alerts`: published likely, scheduled, observed
  rollout, and reported landed alerts, with original source links.
- `https://resetbeacon.com/api/history`: explicit banked/non-banked classification,
  scope, and whether an earlier announcement was fulfilled or superseded. An
  explicitly completed global reset linked to a fulfilled earlier announcement
  still present in the feed can trigger a landing alert before the separate alert
  feed publishes it. Keeping that announcement's event ID preserves old delivery keys.
  Standalone comments, policy changes and promises do not qualify for this fallback.
- Contract: <https://resetbeacon.com/api/docs/>.

Willreset also has public JSON endpoints (`/api/notify`, `/api/timeline`), but
Reset Beacon's published alert feed distinguishes more of the requested stages.
OpenAI Status describes outages, which do not prove that usage will reset.
Neither is polled in addition to Reset Beacon, avoiding conflicting duplicate alerts.

Likely alerts forward the tracker's published prediction or official hint, not a
new probability model. Scheduled alerts report an announcement. An elapsed
deadline never becomes a landed alert. Observed rollouts and official action
claims remain distinct. These are public reports, not verification of Michael's
own account. Each notification includes the type, scope when available, source
name, tracker and original post time when available. The original source URL is
the Pushover **View source** button. Long reply threads and raw archive URLs stay
out of the push body. Unknown types remain
unspecified. Banked alerts explain that the credit must be redeemed to refill usage.
The feed has previously labeled a banked promise as an action. A landed title
therefore requires completed history or measured banked-credit receipt; other
action reports are labeled updates with the source's explanation preserved.

## Delivery and recovery

Only alerts published within the last 48 hours are eligible, including on first
startup. This catches recent news without replaying the historical timeline; a
longer outage can miss older alerts. History fallback uses the announcement time
for this window. Withdrawn alerts, expired hints and previews
already fulfilled are suppressed. Feed snapshots older than 45 minutes fail the
task visibly. HTTP reads have a 20-second timeout and 2 MiB body limit.

Durable `codex-reset-delivery` rows reserve each event/stage/type before Pushover.
Cosmetic revisions do not resend; a changed scheduled time gets a new key.
Source-post aliases are reserved atomically with the event key, so a history
landing followed by a feed alert sends once. Existing delivery keys remain valid.
Reservations expire after 90 days. Confirmed 4xx rejection permits the next poll
to retry; uncertain delivery is retained rather than resent. Check failed runs
and uncertain counts in Omni if a notification appears missing. Provider acceptance
is the available delivery confirmation; it cannot establish that a device displayed it.

Each run logs a bounded source snapshot: fetch time, feed generation time, newest
alert ID, original post time, alert publication time and history fallback count.
These distinguish local polling delay from upstream publication changes. They do
not establish when a tracker first made an event publicly available.

## Latency investigation (2026-10-03)

For post `2106131810921136451`, the original timestamp was October 2 at
21:18:48 UTC. Reset Beacon later reported an alert publication time of 22:42:04
UTC; Omni delivered at October 3 at 05:05 UTC. Earlier scheduled runs succeeded
but had no eligible signals. Old logs did not retain source snapshots, so the
remaining gap cannot be attributed conclusively. One-minute polling removes up
to four minutes of local polling delay; history fallback removes the requirement
to wait for a separate alert publication. Neither guarantees immediate upstream
discovery. Willreset's notify and timeline fields disagreed on this event's
completion state, and its original post timestamp did not prove faster discovery.

## Deployment verification

Pushing main builds and deploys the normal container. No new credentials or
database migration is needed. On Boris, confirm the image revision, inspect the
`CodexResets` startup/scheduled run in `/api/tasks`, and inspect its run summary.
A second run should report existing current signals as already handled, without
another Pushover message. Never erase delivery rows to test duplicate prevention.
