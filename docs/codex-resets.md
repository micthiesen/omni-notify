# Codex reset alerts

`CodexResets` runs at startup and every five minutes (`0 */5 * * * *`). It uses
the existing `PUSHOVER_USER` and `PUSHOVER_TOKEN`; both must be configured for
registration. The task appears in Omni's task list and supports manual runs.

## Sources and meaning

The primary source is Reset Beacon's public JSON API:

- `https://resetbeacon.com/api/alerts`: published likely, scheduled, observed
  rollout, and reported landed alerts, with original source links.
- `https://resetbeacon.com/api/history`: explicit banked/non-banked classification,
  scope, and whether an earlier announcement was fulfilled or superseded.
- Contract: <https://resetbeacon.com/api/docs/>.

Willreset also has public JSON endpoints (`/api/notify`, `/api/timeline`), but
Reset Beacon's published alert feed distinguishes more of the requested stages.
OpenAI Status describes outages, which do not prove that usage will reset.
Neither is polled in addition to Reset Beacon, avoiding conflicting duplicate alerts.

Likely alerts forward the tracker's published prediction or official hint, not a
new probability model. Scheduled alerts report an announcement. An elapsed
deadline never becomes a landed alert. Observed rollouts and official action
claims remain distinct. These are public reports, not verification of Michael's
own account. Each notification includes the type, scope when available, original
source, tracker, and evidence archive when supplied. Unknown types remain
unspecified. Banked alerts explain that the credit must be redeemed to refill usage.
The feed has previously labeled a banked promise as an action. A landed title
therefore requires completed history or measured banked-credit receipt; other
action reports are labeled updates with the source's explanation preserved.

## Delivery and recovery

Only alerts published within the last 48 hours are eligible, including on first
startup. This catches recent news without replaying the historical timeline; a
longer outage can miss older alerts. Withdrawn alerts, expired hints and previews
already fulfilled are suppressed. Feed snapshots older than 45 minutes fail the
task visibly. HTTP reads have a 20-second timeout and 2 MiB body limit.

Durable `codex-reset-delivery` rows reserve each event/stage/type before Pushover.
Cosmetic revisions do not resend; a changed scheduled time gets a new key.
Reservations expire after 90 days. Confirmed 4xx rejection permits the next poll
to retry; uncertain delivery is retained rather than resent. Check failed runs
and uncertain counts in Omni if a notification appears missing. Provider acceptance
is the available delivery confirmation; it cannot establish that a device displayed it.

## Deployment verification

Pushing main builds and deploys the normal container. No new credentials or
database migration is needed. On Boris, confirm the image revision, inspect the
`CodexResets` startup/scheduled run in `/api/tasks`, and inspect its run summary.
A second run should report existing current signals as already handled, without
another Pushover message. Never erase delivery rows to test duplicate prevention.
