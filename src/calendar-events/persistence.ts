import { Entity } from "@micthiesen/mitools/entities";

// Dedup gate: tracks created calendar events by content hash
export type CreatedCalendarEventData = {
  eventHash: string;
  emailId: string;
  calendarEventId: string;
  title: string;
  startDate: string;
  createdAt: number;
};

export const CreatedCalendarEventEntity = new Entity<
  CreatedCalendarEventData,
  ["eventHash"]
>("calendar-created-event", ["eventHash"]);

export function hasCreatedEvent(eventHash: string): boolean {
  return CreatedCalendarEventEntity.get({ eventHash }) !== undefined;
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
  // Simple hash: use a deterministic string as key
  return key;
}
