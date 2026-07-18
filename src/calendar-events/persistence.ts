import { Entity } from "@micthiesen/mitools/entities";
import { toDateStamp } from "../utils/dates.js";

/** Fixed repeat pattern for a recurring event (RRULE FREQ + inclusive UNTIL date). */
export type EventRecurrence = {
  frequency: "daily" | "weekly" | "monthly";
  until: string;
};

// Tracks created calendar events by content hash for dedup and cancel/update matching
export type CreatedCalendarEventData = {
  eventHash: string;
  emailId: string;
  calendarEventId: string;
  title: string;
  startDate: string;
  startTime?: string;
  endDate?: string;
  endTime?: string;
  allDay: boolean;
  location?: string;
  timeZone?: string;
  description?: string;
  duration?: string;
  reminderMinutes?: number;
  recurrence?: EventRecurrence;
  createdAt: number;
  status?: "cancelled";
};

export const CreatedCalendarEventEntity = new Entity<
  CreatedCalendarEventData,
  ["eventHash"]
>("calendar-created-event", ["eventHash"]);

export function hasCreatedEvent(eventHash: string): boolean {
  const record = CreatedCalendarEventEntity.get({ eventHash });
  return record !== undefined && record.status !== "cancelled";
}

export function recordCreatedEvent(data: CreatedCalendarEventData): void {
  CreatedCalendarEventEntity.upsert(data);
}

/**
 * Normalize a title for identity comparison. Strips emoji, arrows, and other
 * punctuation and collapses casing/whitespace, so the same event re-extracted with a
 * slightly different title (e.g. "✈️ Flight YYZ → YVR" vs "Flight YYZ -> YVR") still
 * dedups. Distinct wording still differs, so genuinely different events don't merge.
 */
export function normalizeTitle(title: string): string {
  return title
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s]/gu, " ")
    .replace(/\s+/g, " ")
    .trim();
}

export function computeEventHash(
  title: string,
  startDate: string,
  startTime?: string,
): string {
  const key = `${normalizeTitle(title)}|${startDate}|${startTime ?? "allday"}`;
  return key;
}

/** Pure: active events within a window of 7 days past through `futureDays` ahead. */
export function selectRecentEvents(
  all: CreatedCalendarEventData[],
  futureDays: number,
  now = Date.now(),
): CreatedCalendarEventData[] {
  const pastCutoff = new Date(now);
  pastCutoff.setDate(pastCutoff.getDate() - 7);
  const futureCutoff = new Date(now);
  futureCutoff.setDate(futureCutoff.getDate() + futureDays);

  const pastStr = toDateStamp(pastCutoff.getTime());
  const futureStr = toDateStamp(futureCutoff.getTime());

  return all.filter(
    (e) =>
      e.status !== "cancelled" && e.startDate >= pastStr && e.startDate <= futureStr,
  );
}

/**
 * Get active events for LLM prompt context. The future horizon is a full year:
 * a 90-day horizon once hid a September event announced in April, so the model
 * re-created it five times without ever seeing the earlier rows.
 */
export function getRecentEvents(futureDays = 365): CreatedCalendarEventData[] {
  return selectRecentEvents(CreatedCalendarEventEntity.getAll(), futureDays);
}

/**
 * From active candidates sharing a normalized title, pick the one on `startDate`. If none
 * match the date, fall back to a *lone* candidate only — never arbitrarily pick among
 * several, so a date mismatch can't cancel/update the wrong same-title event.
 */
export function pickByStartDate(
  active: CreatedCalendarEventData[],
  startDate: string,
): CreatedCalendarEventData | undefined {
  const exact = active.find((e) => e.startDate === startDate);
  if (exact) return exact;
  return active.length === 1 ? active[0] : undefined;
}

/**
 * Find an active event by normalized title + startDate within `candidates` — the
 * same windowed set that was shown to the model, never the full store. (Stale
 * past events once broke lone-candidate resolution for a title the model could
 * actually see, degrading an update into a duplicate create.)
 * Falls back to a lone title-only match when no title+date match is found.
 */
export function findEvent(
  title: string,
  startDate: string,
  candidates: CreatedCalendarEventData[],
): CreatedCalendarEventData | undefined {
  const normalized = normalizeTitle(title);
  const active = candidates.filter(
    (e) => e.status !== "cancelled" && normalizeTitle(e.title) === normalized,
  );

  return pickByStartDate(active, startDate);
}

/**
 * Strict resolution via the explicit evt_N handle the model was shown — no
 * title/date fallback. Cancels use this so a title-only match can never delete
 * an event (e.g. a receipt email echoing an upcoming appointment's title).
 */
export function resolveExplicitEventReference(
  ref: { eventId?: string },
  byId: Map<string, CreatedCalendarEventData>,
): CreatedCalendarEventData | undefined {
  if (!ref.eventId) return undefined;
  // Tolerate the model echoing the handle with its surrounding brackets/whitespace
  // (e.g. "[evt_2]") — the map is keyed on the bare "evt_2".
  const handle = ref.eventId.replace(/[^a-z0-9_]/gi, "");
  return byId.get(handle);
}

/**
 * Resolve which stored event an update action refers to. Prefers the stable
 * per-prompt handle (eventId) the model was shown in the existing-events list; falls
 * back to title+startDate matching when the handle is absent or unrecognized (e.g. the
 * model omitted or hallucinated it). The title is thus cosmetic, not the identity key.
 * The fallback searches only the windowed events the model was shown (`byId` values).
 */
export function resolveEventReference(
  ref: { eventId?: string; title: string; startDate: string },
  byId: Map<string, CreatedCalendarEventData>,
  fallback?: (title: string, startDate: string) => CreatedCalendarEventData | undefined,
): CreatedCalendarEventData | undefined {
  const matched = resolveExplicitEventReference(ref, byId);
  if (matched) return matched;
  const resolve =
    fallback ??
    ((title: string, startDate: string) =>
      findEvent(title, startDate, [...byId.values()]));
  return resolve(ref.title, ref.startDate);
}

/** Check if an extracted event has meaningful changes compared to the stored record. */
export function hasEventChanged(
  record: CreatedCalendarEventData,
  event: {
    title: string;
    startDate: string;
    startTime?: string;
    endDate?: string;
    endTime?: string;
    allDay: boolean;
    location?: string;
    timeZone?: string;
    description?: string;
    duration?: string;
    reminderMinutes?: number;
    recurrence?: EventRecurrence | null;
  },
): boolean {
  return (
    normalizeTitle(record.title) !== normalizeTitle(event.title) ||
    record.startDate !== event.startDate ||
    (record.startTime ?? undefined) !== (event.startTime ?? undefined) ||
    (record.endDate ?? undefined) !== (event.endDate ?? undefined) ||
    (record.endTime ?? undefined) !== (event.endTime ?? undefined) ||
    record.allDay !== event.allDay ||
    (record.location ?? undefined) !== (event.location ?? undefined) ||
    (record.timeZone ?? undefined) !== (event.timeZone ?? undefined) ||
    (record.description ?? undefined) !== (event.description ?? undefined) ||
    (record.duration ?? undefined) !== (event.duration ?? undefined) ||
    (record.reminderMinutes ?? undefined) !== (event.reminderMinutes ?? undefined) ||
    recurrenceKey(record.recurrence) !== recurrenceKey(event.recurrence)
  );
}

function recurrenceKey(recurrence: EventRecurrence | null | undefined): string {
  return recurrence ? `${recurrence.frequency}|${recurrence.until}` : "";
}

/**
 * One-time idempotent migration: re-key any stored event whose persisted eventHash
 * predates the current computeEventHash normalization, so create-dedup keeps matching it.
 * Safe to run on every startup — once re-keyed, recomputed hashes match and it's a no-op.
 * Returns the number of rows re-keyed.
 */
export function reconcileEventHashes(): number {
  let rekeyed = 0;
  for (const row of CreatedCalendarEventEntity.getAll()) {
    const expected = computeEventHash(row.title, row.startDate, row.startTime);
    if (row.eventHash === expected) continue;
    CreatedCalendarEventEntity.upsert({ ...row, eventHash: expected });
    CreatedCalendarEventEntity.delete({ eventHash: row.eventHash });
    rekeyed++;
  }
  return rekeyed;
}

/** Mark an existing event as cancelled (preserves record to prevent re-creation). */
export function markEventCancelled(eventHash: string): void {
  const record = CreatedCalendarEventEntity.get({ eventHash });
  if (record) {
    CreatedCalendarEventEntity.upsert({ ...record, status: "cancelled" });
  }
}
