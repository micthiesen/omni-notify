# Calendar

`omni-calendar` has two parts: the email pipeline that turns booking emails
into tracked events, and management of Michael's primary iCloud calendar
through MCP tools and read-only API routes.

## Primary calendar identity

The primary calendar is the single VEVENT collection named "iCloud" in the
CalDAV home set. When the server reports `schedule-default-calendar-URL`, it
must point at the same collection. The resolved collection is pinned durably
(`calendar-primary-pin`) and re-resolved every six hours, after sync errors,
and after a 401 or 403.

- Two or more matches fail closed with `calendar_identity_ambiguous`.
- A server default that names another collection fails with
  `calendar_identity_mismatch`.
- A different, unambiguous match re-pins automatically and logs a warning; the
  pin keeps the previous path hash and `repinnedAt`. There is no manual clear.

`calendar_status` reports the state, whether the calendar is the server
default, writable, sync-capable, and whether the email pipeline writes to it.
It never exposes collection URLs or credentials.

## Mirror and sync

`calendar-primary-resource` mirrors each `.ics` resource with its ETag.
Sync uses RFC 6578 `sync-collection` with the stored token, follows 507
truncation, and fetches changed resources by `calendar-multiget` in batches of
50 (halving a batch that is too large). A rejected token (`valid-sync-token`
403/409) or a server without sync falls back to a PROPFIND ETag diff.

`CalendarPrimarySync` runs every minute and syncs when the mirror is more than
five minutes old (every run while a `calendar.*` MCP Events subscription is
active); reads sync when it is older than 30 seconds or on `fresh`.

## Change feed

Each sync writes `calendar-primary-change` rows with a monotonic sequence key:
`created`, `updated` (with `changedFields` and before/after snapshots) or
`deleted`, and an `origin` of `omni` for Omni's own writes (matched through
`calendar-primary-write-echo`) or `external`. The first sync is a silent
baseline, DTSTAMP-only churn is ignored, and rows are kept for 30 days up to
5,000. `calendar_changes_list` (external changes by default, `origin: any` for
all) and `/api/calendar/changes` (all) page by cursor.

## MCP Events

After each sync, rows past `published_seq` are published as
`calendar.event_changed` (only while subscribed; otherwise the cursor skips
ahead). `CalendarStartingEvents` scans every 30 seconds while a
`calendar.event_starting` subscription is active and fires once per
occurrence, start instant and subscription tuple. Rules and payloads are in
`docs/mcp-events.md`.

## Writes

MCP writes follow one durable path (`calendar-mcp-operation`):

1. Validate and plan the iCalendar bodies against the current resource.
2. Reserve the idempotency key with a fingerprint of the input.
3. Mark each step `sending`, PUT or DELETE with If-Match or If-None-Match,
   and classify the response.
4. Verify by GET and record the echo for the change feed.

A 412 is `version_conflict`; a 403 `no-uid-conflict` is `uid_conflict`. A
transport error or 5xx makes the step `uncertain`: it is reconciled only by
reading the resource, and settles as `not_applied` after ten minutes if the old
content remains (measured from the reservation, so polling
`calendar_write_status` does not postpone it). Uncertain writes are never
resent. A boot step marks
operations interrupted while sending as uncertain and unsent ones as failed. A
`following` split truncates the original with UNTIL, creates a sibling series
(`RELATED-TO;RELTYPE=SIBLING`), and deletes the new series if the truncation
fails. Record mode records writes without sending them.

Invitations (another organizer) are read-only. The role is taken across the
master and every override, so one occurrence with guests counts. Events Michael
organizes with attendees are written only with `attendeeNotifications: "send"`, because iCloud
emails attendees. Scheduling writes beyond that are not implemented.

## Email pipeline updates

The pipeline never overwrites a calendar event wholesale. It GETs the event,
merges only the groups whose extracted values changed (title, time, location,
notes, recurrence, and its own alarm), and PUTs with If-Match. A 412 re-reads
and merges again, up to three attempts. A 404 recreates with If-None-Match. A
403 `no-uid-conflict` on the trusted iCloud host means Michael moved the event
to another calendar; the merge targets that copy in place.

A create for a title that matches exactly one active tracked event updates that
event instead of creating a duplicate when it is on the same day at another
time (non-recurring), or on another date within 45 days and the email mentions
a reschedule, postponement, or new date or time. An email that lists the same
title more than once never reschedules (it describes several bookings).

## API

`omni-api::calendar` DTOs back `GET /api/calendar/status`,
`/api/calendar/events?from&to`, and `/api/calendar/changes?cursor&limit`.
