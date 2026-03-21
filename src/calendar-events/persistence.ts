import { Entity } from "@micthiesen/mitools/entities";

// Tracks created calendar events by content hash for dedup and cancel/update matching
export type CreatedCalendarEventData = {
  eventHash: string;
  emailId: string;
  calendarEventId: string;
  title: string;
  startDate: string;
  startTime?: string;
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

export function computeEventHash(
  title: string,
  startDate: string,
  startTime?: string,
): string {
  const key = `${title.toLowerCase().trim()}|${startDate}|${startTime ?? "allday"}`;
  return key;
}

/** Get active events within a window: 7 days past through `futureDays` ahead (for LLM prompt context). */
export function getRecentEvents(futureDays = 90): CreatedCalendarEventData[] {
  const all = CreatedCalendarEventEntity.getAll();
  const now = new Date();
  const pastCutoff = new Date(now);
  pastCutoff.setDate(pastCutoff.getDate() - 7);
  const futureCutoff = new Date(now);
  futureCutoff.setDate(futureCutoff.getDate() + futureDays);

  const pastStr = pastCutoff.toISOString().slice(0, 10);
  const futureStr = futureCutoff.toISOString().slice(0, 10);

  return all.filter(
    (e) =>
      e.status !== "cancelled" && e.startDate >= pastStr && e.startDate <= futureStr,
  );
}

/**
 * Find an active event by title + startDate (case-insensitive title).
 * Matches on both to handle recurring events with the same title on different dates.
 * Falls back to title-only if no title+date match is found.
 */
export function findEvent(
  title: string,
  startDate: string,
): CreatedCalendarEventData | undefined {
  const all = CreatedCalendarEventEntity.getAll();
  const normalized = title.toLowerCase().trim();
  const active = all.filter(
    (e) => e.status !== "cancelled" && e.title.toLowerCase().trim() === normalized,
  );

  return active.find((e) => e.startDate === startDate) ?? active[0];
}

/** Mark an existing event as cancelled (preserves record to prevent re-creation). */
export function markEventCancelled(eventHash: string): void {
  const record = CreatedCalendarEventEntity.get({ eventHash });
  if (record) {
    CreatedCalendarEventEntity.upsert({ ...record, status: "cancelled" });
  }
}
