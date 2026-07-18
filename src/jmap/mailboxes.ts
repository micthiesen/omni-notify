import type { Logger } from "@micthiesen/mitools/logging";
import type { JmapContext } from "./client.js";

/** Mailbox id -> RFC 8621 role ("inbox", "sent", "junk", ...) or null. */
export type MailboxRoles = ReadonlyMap<string, string | null>;

/**
 * Roles whose mail the pipelines should process. Everything else (sent,
 * drafts, junk/spam, trash, custom folders) is skipped — a June incident had
 * sent + spam mail flowing through the parcel/calendar pipelines.
 */
const ALLOWED_ROLES = new Set(["inbox", "archive"]);

// Refresh periodically: this is a long-running daemon, and a mailbox created
// or re-roled after boot would otherwise be unknown for the process lifetime
// (unknown ids fail open, so drift quietly widens what gets processed).
const ROLES_TTL_MS = 6 * 60 * 60_000;

const rolesCache = new WeakMap<
  JmapContext,
  { roles: MailboxRoles; fetchedAt: number }
>();

/**
 * Resolve the account's mailbox id -> role map via Mailbox/get, cached in
 * memory per JmapContext (fetched lazily, refreshed every few hours). Returns
 * undefined when resolution fails so callers can fail open — losing parcels
 * is worse than processing extra mail.
 */
export async function getMailboxRoles(
  ctx: JmapContext,
  logger: Logger,
): Promise<MailboxRoles | undefined> {
  const cached = rolesCache.get(ctx);
  if (cached && Date.now() - cached.fetchedAt < ROLES_TTL_MS) return cached.roles;

  try {
    const [result] = await ctx.jam.request([
      "Mailbox/get",
      // Omitting `ids` fetches all mailboxes (JMAP defaults ids to null).
      { accountId: ctx.accountId, properties: ["id", "role"] },
    ]);
    const list = (result as Record<string, unknown>).list as
      | { id: string; role?: string | null }[]
      | undefined;
    if (!list) throw new Error("Mailbox/get returned no list");

    const roles = new Map<string, string | null>(
      list.map((m) => [m.id, m.role ?? null]),
    );
    rolesCache.set(ctx, { roles, fetchedAt: Date.now() });
    logger.debug(`Resolved ${roles.size} mailbox role(s)`);
    return roles;
  } catch (error) {
    logger.warn(
      `Failed to resolve mailbox roles, processing all emails: ${(error as Error).message}`,
    );
    // A stale snapshot beats no snapshot when the refresh fails.
    return cached?.roles;
  }
}

/**
 * Pure: should an email in these mailboxes be processed? Allowed when any of
 * its mailboxes has role "inbox" or "archive". Fails open (true) when role
 * resolution failed, mailbox info is missing, or every mailbox id is unknown
 * to our snapshot (e.g. a mailbox created after the cache was populated).
 */
export function isEmailInAllowedMailbox(
  mailboxIds: Record<string, boolean> | undefined,
  roles: MailboxRoles | undefined,
): boolean {
  if (roles === undefined) return true;
  if (mailboxIds === undefined) return true;

  const ids = Object.keys(mailboxIds).filter((id) => mailboxIds[id]);
  if (ids.length === 0) return true;

  let anyKnown = false;
  for (const id of ids) {
    if (!roles.has(id)) continue;
    anyKnown = true;
    const role = roles.get(id);
    if (role !== null && role !== undefined && ALLOWED_ROLES.has(role)) return true;
  }
  return !anyKnown;
}
