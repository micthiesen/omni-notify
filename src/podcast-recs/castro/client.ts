import type { Logger } from "@micthiesen/mitools/logging";
import type { PodcastAccountClient } from "../account.js";

/**
 * Castro bridge — NOT YET IMPLEMENTED.
 *
 * Castro has no public API and no username/password login: its device sync
 * stores queue/episode state on Castro's servers, authenticated by
 * per-installation credentials that live in the user's iCloud Keychain.
 * Read docs/castro-sync.md before implementing — it captures everything
 * known about the sync architecture, the candidate approaches for obtaining
 * credentials, and the fallbacks the pipeline uses in the meantime.
 *
 * Implement by returning an object satisfying PodcastAccountClient (see
 * src/podcast-recs/account.ts for the contract, especially the
 * unavailable-vs-empty rule) and gate on whatever config the transport needs
 * (e.g. CASTRO_CREDENTIALS in utils/config.ts), returning null when
 * unconfigured so the pipeline keeps using its fallbacks.
 */
export function createCastroClient(_logger: Logger): PodcastAccountClient | null {
  return null;
}
