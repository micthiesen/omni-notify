/** Formats an epoch as a UTC YYYY-MM-DD date stamp. */
export function toDateStamp(epochMs = Date.now()): string {
  return new Date(epochMs).toISOString().slice(0, 10);
}
