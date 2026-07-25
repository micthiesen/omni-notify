/** Formats an epoch as a UTC YYYY-MM-DD date stamp. */
export function toDateStamp(epochMs = Date.now()): string {
  return new Date(epochMs).toISOString().slice(0, 10);
}

/** Compact elapsed-time label for alert copy: "1m", "45m", "2h", "2h15m". */
export function formatElapsed(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 60) return `${Math.max(minutes, 1)}m`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest > 0 ? `${hours}h${rest}m` : `${hours}h`;
}
