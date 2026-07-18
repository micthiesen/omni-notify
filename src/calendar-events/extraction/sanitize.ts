import type { ExtractedCalendarEvent } from "./schema.js";

export const MAX_TITLE_CHARS = 200;
export const MAX_LOCATION_CHARS = 300;
export const MAX_DESCRIPTION_CHARS = 2000;

export interface SanitizeResult {
  events: ExtractedCalendarEvent[];
  /** Human-readable notes on everything that was fixed (one warn line each). */
  issues: string[];
  duplicatesCollapsed: number;
  timeZonesDropped: number;
}

let supportedTimeZones: Set<string> | undefined;

function getSupportedTimeZones(): Set<string> {
  if (!supportedTimeZones) {
    try {
      supportedTimeZones = new Set(Intl.supportedValuesOf("timeZone"));
    } catch {
      supportedTimeZones = new Set();
    }
  }
  return supportedTimeZones;
}

/** True if `tz` is a real IANA zone name this runtime can resolve. */
export function isValidTimeZone(tz: string): boolean {
  if (getSupportedTimeZones().has(tz)) return true;
  // Aliases (e.g. Asia/Calcutta) resolve via DateTimeFormat without appearing in
  // supportedValuesOf; garbage (model field soup) throws a RangeError instead.
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: tz });
    return true;
  } catch {
    return false;
  }
}

/**
 * A valid zone passes through unchanged; anything else — including whole
 * paragraphs of model field soup — becomes undefined so it can never reach a
 * DTSTART;TZID= line or be re-injected into a later prompt.
 */
export function sanitizeTimeZone(tz: string | undefined): string | undefined {
  if (tz === undefined) return undefined;
  return isValidTimeZone(tz) ? tz : undefined;
}

export function truncated(value: string, max: number): string {
  return value.length > max ? value.slice(0, max) : value;
}

/** JSON with sorted keys and undefineds dropped, for byte-identity comparison. */
function stableStringify(value: unknown): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(",")}]`;
  const entries = Object.entries(value as Record<string, unknown>)
    .filter(([, v]) => v !== undefined)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([k, v]) => `${JSON.stringify(k)}:${stableStringify(v)}`);
  return `{${entries.join(",")}}`;
}

function label(event: ExtractedCalendarEvent): string {
  return `"${truncated(event.title, 40)}"`;
}

/**
 * Post-model output sanitization. The extraction model has degenerated in
 * production (hundreds of identical create objects, paragraphs of field soup in
 * timeZone, timed events with no startTime), so every model response passes
 * through here before any of it touches CalDAV or persistence.
 */
export function sanitizeExtractedEvents(
  events: ExtractedCalendarEvent[],
): SanitizeResult {
  const issues: string[] = [];
  let timeZonesDropped = 0;

  // Collapse byte-identical duplicates (degenerate repetition) to one.
  const seen = new Set<string>();
  const unique: ExtractedCalendarEvent[] = [];
  for (const event of events) {
    const key = stableStringify(event);
    if (seen.has(key)) continue;
    seen.add(key);
    unique.push(event);
  }
  const duplicatesCollapsed = events.length - unique.length;
  if (duplicatesCollapsed > 0) {
    issues.push(`collapsed ${duplicatesCollapsed} byte-identical duplicate event(s)`);
  }

  const sanitized = unique.map((event) => {
    const out: ExtractedCalendarEvent = { ...event };

    if (out.timeZone !== undefined && !isValidTimeZone(out.timeZone)) {
      issues.push(
        `dropped invalid timeZone (${out.timeZone.length} chars) on ${label(out)}`,
      );
      timeZonesDropped++;
      out.timeZone = undefined;
    }

    if (out.title.length > MAX_TITLE_CHARS) {
      issues.push(`truncated title (${out.title.length} → ${MAX_TITLE_CHARS} chars)`);
      out.title = truncated(out.title, MAX_TITLE_CHARS);
    }
    if (out.location !== undefined && out.location.length > MAX_LOCATION_CHARS) {
      issues.push(
        `truncated location (${out.location.length} → ${MAX_LOCATION_CHARS} chars) on ${label(out)}`,
      );
      out.location = truncated(out.location, MAX_LOCATION_CHARS);
    }
    if (
      out.description !== undefined &&
      out.description.length > MAX_DESCRIPTION_CHARS
    ) {
      issues.push(
        `truncated description (${out.description.length} → ${MAX_DESCRIPTION_CHARS} chars) on ${label(out)}`,
      );
      out.description = truncated(out.description, MAX_DESCRIPTION_CHARS);
    }

    // A timed event with no startTime would become a midnight event with an
    // 11:30 PM alarm the night before — force it to all-day instead.
    if (!out.allDay && out.startTime === undefined) {
      issues.push(`forced allDay on ${label(out)} (timed event with no startTime)`);
      out.allDay = true;
      out.endTime = undefined;
      out.duration = undefined;
    }

    if (out.recurrence === null) out.recurrence = undefined;
    if (out.recurrence && !/^\d{4}-\d{2}-\d{2}$/.test(out.recurrence.until)) {
      issues.push(
        `dropped recurrence with invalid until "${truncated(out.recurrence.until, 40)}" on ${label(out)}`,
      );
      out.recurrence = undefined;
    }

    return out;
  });

  return { events: sanitized, issues, duplicatesCollapsed, timeZonesDropped };
}

/**
 * True when the model output looks degenerate enough to warrant one fresh
 * retry: more than half the returned objects were duplicates, or a timeZone
 * had to be dropped (field soup strongly correlates with a bad sample).
 */
export function isDegenerateExtraction(result: SanitizeResult): boolean {
  const rawCount = result.events.length + result.duplicatesCollapsed;
  if (result.timeZonesDropped > 0) return true;
  return rawCount > 0 && result.duplicatesCollapsed > rawCount / 2;
}
