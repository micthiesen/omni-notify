/**
 * URL identity for PressPods: two submissions that point at the same article
 * should collapse to one episode instead of racing into duplicate rows. We
 * normalize only for *identity/dedup* — the original URL is still what the
 * retrievers fetch, since some sites (substack redirect links, paywall
 * bouncers) genuinely need their query string to resolve.
 *
 * Normalization is deliberately conservative: strip known tracking params and
 * fragments, lowercase the host, drop a leading `www.`, and canonicalize the
 * trailing slash. We do NOT strip every query param — plenty of sites carry
 * article identity in the query string (`?p=123`, `?story=…`), and dropping
 * those would merge distinct articles.
 */

/** Query params that never affect which article a URL points at. */
const TRACKING_PARAMS = new Set([
  "ref",
  "r",
  "fbclid",
  "gclid",
  "dclid",
  "gbraid",
  "wbraid",
  "msclkid",
  "yclid",
  "mc_cid",
  "mc_eid",
  "igshid",
  "igsh",
  "si",
  "triedRedirect",
  "source",
  "spm",
  "_hsenc",
  "_hsmi",
  "vero_id",
  "vero_conv",
  "oly_anon_id",
  "oly_enc_id",
  "s_cid",
  "cmpid",
  "ncid",
  "mkt_tok",
  "guccounter",
  "showWelcomeOnShare",
]);

function isTrackingParam(key: string): boolean {
  const lower = key.toLowerCase();
  return (
    lower.startsWith("utm_") || TRACKING_PARAMS.has(key) || TRACKING_PARAMS.has(lower)
  );
}

/**
 * Canonical identity for a submitted article URL. Falls back to the trimmed
 * input when the string doesn't parse as a URL (the submit schema already
 * requires a valid URL, so this is just defensive).
 */
export function normalizeUrl(raw: string): string {
  const trimmed = raw.trim();
  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    return trimmed;
  }

  // Identity ignores the scheme (http/https links to the same article are the
  // same piece — a common duplicate source from old bookmarks/RSS), a leading
  // `www.`, and a trailing DNS-root dot on the host.
  url.protocol = "https:";
  url.hostname = url.hostname
    .toLowerCase()
    .replace(/^www\./, "")
    .replace(/\.$/, "");
  url.hash = "";

  const kept: [string, string][] = [];
  for (const [key, value] of url.searchParams) {
    if (!isTrackingParam(key)) kept.push([key, value]);
  }
  // Sort for a stable identity regardless of param order in the source link.
  kept.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  url.search = "";
  for (const [key, value] of kept) url.searchParams.append(key, value);

  // Canonicalize the trailing slash (but keep the root path as "/").
  if (url.pathname.length > 1 && url.pathname.endsWith("/")) {
    url.pathname = url.pathname.replace(/\/+$/, "");
  }

  return url.toString();
}
