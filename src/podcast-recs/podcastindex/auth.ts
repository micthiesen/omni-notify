import { createHash } from "node:crypto";

export interface PodcastIndexCredentials {
  key: string;
  secret: string;
}

const USER_AGENT = "omni-notify/1.0";

/** sha1(apiKey + apiSecret + authDateSeconds) as lowercase hex, per Podcast Index's auth scheme. */
export function podcastIndexAuthHash(
  key: string,
  secret: string,
  authDateSeconds: string,
): string {
  return createHash("sha1")
    .update(key + secret + authDateSeconds)
    .digest("hex");
}

export function podcastIndexAuthHeaders(
  creds: PodcastIndexCredentials,
  nowMs = Date.now(),
): Record<string, string> {
  const authDateSeconds = String(Math.floor(nowMs / 1000));
  return {
    "X-Auth-Key": creds.key,
    "X-Auth-Date": authDateSeconds,
    Authorization: podcastIndexAuthHash(creds.key, creds.secret, authDateSeconds),
    "User-Agent": USER_AGENT,
  };
}
