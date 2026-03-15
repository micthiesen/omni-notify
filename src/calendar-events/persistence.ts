import { Entity } from "@micthiesen/mitools/entities";

// Tracks JMAP email state for the calendar pipeline (separate from parcel-tracker)
export type CalendarEmailStateData = {
  key: "singleton";
  state: string;
  updatedAt: number;
};

export const CalendarEmailStateEntity = new Entity<CalendarEmailStateData, ["key"]>(
  "calendar-email-state",
  ["key"],
);

export function getCalendarEmailState(): string | undefined {
  return CalendarEmailStateEntity.get({ key: "singleton" })?.state;
}

export function saveCalendarEmailState(state: string): void {
  CalendarEmailStateEntity.upsert({
    key: "singleton",
    state,
    updatedAt: Date.now(),
  });
}

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
