export const ALERT_LOOKBACK_MS = 48 * 60 * 60_000;
export const CLOCK_SKEW_MS = 5 * 60_000;

export const pacificTime = (value: string) =>
  new Intl.DateTimeFormat("en-CA", {
    timeZone: "America/Vancouver",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    timeZoneName: "short",
  }).format(new Date(value));

/** Push previews should contain the news, not a clipped reply thread or raw URLs. */
export function compactSummary(text: string): string {
  const clean = text
    .replace(/https?:\/\/\S+/g, "")
    .replace(/\s+/g, " ")
    .trim();
  if (clean.length <= 200) return clean;
  const prefix = clean.slice(0, 197);
  const space = prefix.lastIndexOf(" ");
  return `${prefix.slice(0, space > 150 ? space : 197).trimEnd()}…`;
}
