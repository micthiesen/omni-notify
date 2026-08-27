const MAX_REDIRECTS = 5;
export const CALDAV_REQUEST_TIMEOUT_MS = 15_000;

function isIcloudCaldavHost(hostname: string): boolean {
  return (
    hostname === "caldav.icloud.com" || /^p\d+-caldav\.icloud\.com$/i.test(hostname)
  );
}

/** Reject URLs that could disclose a provider password to another origin. */
export function assertTrustedCaldavUrl(
  url: string,
  provider: "icloud" | "fastmail",
): string {
  const parsed = new URL(url);
  const trusted =
    parsed.protocol === "https:" &&
    (provider === "icloud"
      ? isIcloudCaldavHost(parsed.hostname)
      : parsed.hostname === "caldav.fastmail.com");
  if (!trusted) {
    throw new Error(`Untrusted ${provider} CalDAV URL: ${parsed.origin}`);
  }
  return parsed.toString();
}

function isSafeRedirect(from: URL, to: URL): boolean {
  if (to.protocol !== "https:") return false;
  if (from.hostname === to.hostname) return true;
  return isIcloudCaldavHost(from.hostname) && isIcloudCaldavHost(to.hostname);
}

/**
 * Issue a CalDAV PROPFIND, following redirects manually — fetch() won't
 * replay a PROPFIND across a redirect, and iCloud's discovery chain redirects
 * from caldav.icloud.com to a per-account pXX-caldav.icloud.com shard.
 * Returns the multistatus XML plus the URL that finally answered (needed to
 * resolve relative hrefs against the right host).
 */
export async function propfind(
  url: string,
  authHeader: string,
  depth: "0" | "1",
  body: string,
): Promise<{ xml: string; url: string }> {
  let current = url;
  for (let hop = 0; hop <= MAX_REDIRECTS; hop++) {
    const response = await fetch(current, {
      method: "PROPFIND",
      redirect: "manual",
      headers: {
        "Content-Type": "application/xml; charset=utf-8",
        Authorization: authHeader,
        Depth: depth,
      },
      body,
      signal: AbortSignal.timeout(CALDAV_REQUEST_TIMEOUT_MS),
    });

    if (response.status >= 300 && response.status < 400) {
      const location = response.headers.get("location");
      if (!location) {
        throw new Error(`CalDAV PROPFIND redirect without Location (${current})`);
      }
      const next = new URL(location, current);
      if (!isSafeRedirect(new URL(current), next)) {
        throw new Error(
          `Refusing to forward CalDAV credentials across redirect (${new URL(current).origin} -> ${next.origin})`,
        );
      }
      current = next.toString();
      continue;
    }

    if (!response.ok) {
      const text = await response.text();
      throw new Error(
        `CalDAV PROPFIND failed: ${response.status} ${response.statusText} (${current})\n${text}`,
      );
    }

    return { xml: await response.text(), url: current };
  }
  throw new Error(`CalDAV PROPFIND exceeded ${MAX_REDIRECTS} redirects (${url})`);
}

export function basicAuth(username: string, password: string): string {
  return `Basic ${Buffer.from(`${username}:${password}`).toString("base64")}`;
}
