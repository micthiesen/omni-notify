import type { Logger } from "@micthiesen/mitools/logging";
import { ImapFlow } from "imapflow";
import { simpleParser } from "mailparser";
import { getLastDispatchedAt } from "../persistence.js";
import type {
  DownloadedAttachment,
  EmailAttachment,
  EmailPoll,
  EmailSearchOptions,
  EmailTransport,
  FetchedEmail,
} from "../types.js";
import {
  type AutoReadClient,
  discoverAutoReadFolders,
  markRecentUnreadRead,
} from "./autoRead.js";
import {
  decodeAttachmentBlobId,
  type MessageCoords,
  mapParsedMessage,
} from "./mapMessage.js";
import { getFolderCursor, saveFolderCursor } from "./persistence.js";
import { planFolderSync } from "./sync.js";

const IMAP_HOST = "imap.mail.me.com";
const IMAP_PORT = 993;

/**
 * Folders whose new mail feeds the pipelines — the IMAP equivalent of the
 * JMAP inbox/archive mailbox-role scoping. Archive is watched directly
 * because iCloud server-side rules can file mail there before IMAP delivery.
 */
const FOLDERS = ["INBOX", "Archive"];

/** IDLE only pushes for the selected folder (INBOX); this sweep catches mail
 * filed straight into Archive and any pushes lost across reconnects. */
const SWEEP_INTERVAL_MS = 5 * 60_000;

/** Re-issue IDLE well before the RFC 2177 29-minute limit. */
const MAX_IDLE_TIME_MS = 13 * 60_000;

// Reconnect backoff mirrors the JMAP EventSource: first attempt jittered
// 0-3s, then doubling, capped at 5 min, reset on successful connect.
const INITIAL_BACKOFF_MS = 1_000;
const MAX_BACKOFF_MS = 5 * 60_000;

/** Safety cap on messages fetched per folder per pass (the cursor only
 * advances past what was fetched, so the rest follows next pass). */
const MAX_EMAILS_PER_PASS = 200;

/**
 * Messages older than this are cursor-skipped without dispatch. Protects the
 * pipelines (and the triage LLM bill) from bulk imports: an imapsync sweep
 * copies years of mail in with brand-new UIDs but preserved INTERNALDATEs.
 */
const MAX_EMAIL_AGE_MS = 7 * 24 * 60 * 60_000;

interface ImapAuth {
  user: string;
  pass: string;
}

/**
 * iCloud transport: IMAP IDLE push + per-folder UID-cursor delta fetch.
 *
 * iCloud server quirks handled here (see prompt research / MailKit #970,
 * Bugzilla #1611624): the pre-login CAPABILITY response is minimal, so
 * capabilities are only trusted post-login (imapflow re-reads them);
 * parameterized `SELECT (CONDSTORE)` is rejected, so QRESYNC stays off and
 * sync is plain-UID based; there is no MOVE, and the only mailbox mutation is
 * adding the \Seen flag for the small auto-read cleanup pass.
 */
export class ImapTransport implements EmailTransport {
  public readonly name = "IMAP";

  private auth: ImapAuth;
  private logger: Logger;
  private client: ImapFlow | null = null;
  private onMailEvent: (() => void) | undefined;
  private sweepTimer: ReturnType<typeof setInterval> | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private backoffMs = 0;
  private stopped = false;
  /** Special-use mailboxes are rediscovered after every new connection. */
  private autoReadFolders: string[] | undefined;
  /** Last bulk-import-guard skip count per folder, to de-noise repeat logs. */
  private lastSkipCounts = new Map<string, number>();

  constructor(auth: ImapAuth, logger: Logger) {
    this.auth = auth;
    this.logger = logger;
  }

  async start(onMailEvent: () => void): Promise<void> {
    this.onMailEvent = onMailEvent;
    // First connect throws so the boot retry loop can alert and re-attempt;
    // later disconnects self-heal via scheduleReconnect.
    await this.connect();
    this.sweepTimer = setInterval(() => this.onMailEvent?.(), SWEEP_INTERVAL_MS);
    onMailEvent();
  }

  stop(): void {
    this.stopped = true;
    if (this.sweepTimer) clearInterval(this.sweepTimer);
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.logger.info("Closing IMAP connection");
    void this.client?.logout().catch(() => this.client?.close());
  }

  private async connect(): Promise<void> {
    const client = new ImapFlow({
      host: IMAP_HOST,
      port: IMAP_PORT,
      secure: true,
      auth: this.auth,
      logger: false,
      maxIdleTime: MAX_IDLE_TIME_MS,
      // qresync deliberately off: iCloud rejects `SELECT ... (CONDSTORE)`.
    });

    client.on("error", (error: Error) => {
      this.logger.warn(`IMAP connection error: ${error.message}`);
    });
    client.on("close", () => {
      if (this.client === client) this.client = null;
      if (!this.stopped) this.scheduleReconnect();
    });
    // New message in the selected mailbox (INBOX) while idling.
    client.on("exists", () => this.onMailEvent?.());

    await client.connect();
    this.autoReadFolders = undefined;
    const caps = ["IDLE", "CONDSTORE", "QRESYNC", "UIDPLUS"]
      .map((c) => `${c}=${client.capabilities.has(c) ? "y" : "n"}`)
      .join(" ");
    this.logger.info(`IMAP connected to ${IMAP_HOST} (${caps})`);

    // imapflow auto-idles on the selected mailbox whenever no command runs.
    await client.mailboxOpen("INBOX", { readOnly: true });
    this.client = client;
    this.backoffMs = INITIAL_BACKOFF_MS;
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer || this.stopped) return;

    if (this.backoffMs === 0) {
      this.backoffMs = Math.round(Math.random() * 3_000);
    } else {
      this.backoffMs = Math.min(this.backoffMs * 2, MAX_BACKOFF_MS);
    }

    this.logger.warn(`IMAP connection closed, reconnecting in ${this.backoffMs}ms`);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      void this.connect()
        .then(() => {
          // Pushes during the gap are gone; treat reconnect as a mail event.
          this.onMailEvent?.();
        })
        .catch((error: Error) => {
          this.logger.warn(`IMAP reconnect failed: ${error.message}`);
          this.scheduleReconnect();
        });
    }, this.backoffMs);
  }

  private requireClient(): ImapFlow {
    if (!this.client?.usable) {
      throw new Error("IMAP connection is not available");
    }
    return this.client;
  }

  async pollNewEmails(): Promise<EmailPoll> {
    const client = this.requireClient();
    const emails: FetchedEmail[] = [];
    const commits: (() => void)[] = [];

    try {
      for (const folder of FOLDERS) {
        try {
          const result = await this.pollFolder(client, folder);
          emails.push(...result.emails);
          if (result.commit) commits.push(result.commit);
        } catch (error) {
          this.logger.warn(
            `IMAP poll failed for folder "${folder}": ${(error as Error).message}`,
          );
        }
      }
      await this.autoRead(client);
    } finally {
      await this.restoreInbox(client);
    }

    return {
      emails,
      commit: () => {
        for (const commit of commits) commit();
      },
    };
  }

  private async autoRead(client: ImapFlow): Promise<void> {
    if (this.autoReadFolders === undefined) {
      try {
        this.autoReadFolders = await discoverAutoReadFolders(client as AutoReadClient);
      } catch (error) {
        this.logger.warn(
          `IMAP auto-read mailbox discovery failed: ${(error as Error).message}`,
        );
        return;
      }
    }

    await markRecentUnreadRead(
      client as AutoReadClient,
      this.autoReadFolders,
      this.logger,
    );
  }

  private async pollFolder(
    client: ImapFlow,
    folder: string,
  ): Promise<{ emails: FetchedEmail[]; commit?: () => void }> {
    const status = await client.status(folder, { uidNext: true, uidValidity: true });
    if (status.uidNext === undefined || status.uidValidity === undefined) {
      throw new Error("STATUS returned no uidNext/uidValidity");
    }
    const uidValidity = String(status.uidValidity);
    const uidNext = status.uidNext;

    const cursor = getFolderCursor(folder);
    const plan = planFolderSync(cursor, { uidValidity, uidNext });

    switch (plan.action) {
      case "init":
        this.logger.info(
          `First run for ${folder}: cursor initialized at uid ${uidNext} (skipping history)`,
        );
        return {
          emails: [],
          commit: () => saveFolderCursor(folder, uidValidity, uidNext),
        };

      case "none":
        return { emails: [] };

      case "reset":
        return this.recoverFromUidValidityChange(client, folder, uidValidity, uidNext);

      case "fetch": {
        const { emails, nextUid } = await this.fetchNewInFolder(
          client,
          folder,
          uidValidity,
          plan.fromUid,
          uidNext,
        );
        return {
          emails,
          commit: () => saveFolderCursor(folder, uidValidity, nextUid),
        };
      }
    }
  }

  private async fetchNewInFolder(
    client: ImapFlow,
    folder: string,
    uidValidity: string,
    fromUid: number,
    statusUidNext: number,
  ): Promise<{ emails: FetchedEmail[]; nextUid: number }> {
    const lock = await client.getMailboxLock(folder, { readOnly: true });
    try {
      // Phase 1: cheap metadata scan of the new-UID range, so bulk imports of
      // old mail can be cursor-skipped without downloading full bodies.
      const metas: { uid: number; internalDate?: Date }[] = [];
      for await (const msg of client.fetch(
        `${fromUid}:*`,
        { internalDate: true },
        { uid: true },
      )) {
        // `${fromUid}:*` returns the highest-UID message even when nothing is
        // new (IMAP range quirk); drop anything below the cursor.
        if (msg.uid >= fromUid) {
          metas.push({ uid: msg.uid, internalDate: toDate(msg.internalDate) });
        }
      }
      metas.sort((a, b) => a.uid - b.uid);

      const cutoff = Date.now() - MAX_EMAIL_AGE_MS;
      const fresh = metas.filter(
        (m) => m.internalDate === undefined || m.internalDate.getTime() >= cutoff,
      );
      if (fresh.length < metas.length) {
        // During an imapsync backfill this fires on every sweep for weeks; only
        // the first occurrence and count changes are worth surfacing at info.
        const skipped = metas.length - fresh.length;
        const level = this.lastSkipCounts.get(folder) === skipped ? "debug" : "info";
        this.lastSkipCounts.set(folder, skipped);
        this.logger[level](
          `${folder}: skipping ${skipped} message(s) older ` +
            `than ${MAX_EMAIL_AGE_MS / 86_400_000}d (bulk import guard)`,
        );
      } else {
        this.lastSkipCounts.delete(folder);
      }

      const selected = fresh.slice(0, MAX_EMAILS_PER_PASS);
      if (fresh.length > selected.length) {
        this.logger.warn(
          `${folder}: fetch pass hit the ${MAX_EMAILS_PER_PASS}-email cap; ` +
            "the rest follows on the next pass",
        );
      }

      const emails: FetchedEmail[] = [];
      for (const meta of selected) {
        const full = orUndefined(
          await client.fetchOne(
            String(meta.uid),
            { source: true, internalDate: true },
            { uid: true },
          ),
        );
        if (!full?.source) continue;
        const parsed = await simpleParser(full.source);
        const email = mapParsedMessage(
          parsed,
          { folder, uidValidity, uid: meta.uid },
          toDate(full.internalDate) ?? meta.internalDate,
        );
        this.logger.debug(
          `Email: "${email.subject}" from=${email.from} uid=${meta.uid} ` +
            `folder=${folder} attachments=${email.attachments.length}`,
        );
        emails.push(email);
      }

      // Advance past everything we consumed or age-skipped; when capped, only
      // up to the last selected message so the remainder isn't lost.
      const nextUid =
        fresh.length > selected.length && selected.length > 0
          ? selected[selected.length - 1].uid + 1
          : Math.max(statusUidNext, (metas.at(-1)?.uid ?? 0) + 1);

      this.logger.debug(`Fetched ${emails.length} new email(s) from ${folder}`);
      return { emails, nextUid };
    } finally {
      lock.release();
    }
  }

  /**
   * UIDVALIDITY changed: every stored UID is meaningless. Mirror the JMAP
   * cannotCalculateChanges recovery — re-dispatch mail received since the
   * last dispatch (pipelines dedup) and re-seat the cursor.
   */
  private async recoverFromUidValidityChange(
    client: ImapFlow,
    folder: string,
    uidValidity: string,
    uidNext: number,
  ): Promise<{ emails: FetchedEmail[]; commit?: () => void }> {
    const lastDispatchedAt = getLastDispatchedAt();
    const commit = () => saveFolderCursor(folder, uidValidity, uidNext);

    if (lastDispatchedAt === undefined) {
      this.logger.warn(
        `${folder}: UIDVALIDITY changed with no last-dispatch timestamp; resetting cursor only`,
      );
      return { emails: [], commit };
    }

    const since = new Date(lastDispatchedAt - 60 * 60_000);
    const lock = await client.getMailboxLock(folder, { readOnly: true });
    let uids: number[] = [];
    try {
      const found = await client.search({ since }, { uid: true });
      if (Array.isArray(found)) uids = found;
    } finally {
      lock.release();
    }

    const cursorForFetch = uids.length > 0 ? Math.min(...uids) : uidNext;
    const { emails } = await this.fetchNewInFolder(
      client,
      folder,
      uidValidity,
      cursorForFetch,
      uidNext,
    );
    this.logger.warn(
      `${folder}: UIDVALIDITY changed; recovered ${emails.length} email(s) ` +
        `received since ${since.toISOString()}`,
    );
    return { emails, commit };
  }

  async fetchEmailById(id: string): Promise<FetchedEmail | undefined> {
    const client = this.requireClient();
    const coords = decodeMessageId(id);

    try {
      for (const folder of FOLDERS) {
        if (coords && coords.folder !== folder) continue;
        const email = await this.findInFolder(client, folder, id, coords);
        if (email) return email;
      }
      return undefined;
    } finally {
      await this.restoreInbox(client);
    }
  }

  async searchEmails(options: EmailSearchOptions): Promise<FetchedEmail[]> {
    const client = this.requireClient();
    const folderNames =
      options.folder === "inbox"
        ? ["INBOX"]
        : options.folder === "archive"
          ? ["Archive"]
          : FOLDERS;
    const perFolderLimit = Math.min(Math.max(1, options.limit), 50);
    const emails: FetchedEmail[] = [];

    try {
      for (const folder of folderNames) {
        const lock = await client.getMailboxLock(folder, { readOnly: true });
        try {
          const criteria = {
            ...(options.query ? { text: options.query } : {}),
            ...(options.from ? { from: options.from } : {}),
            ...(options.to ? { to: options.to } : {}),
            ...(options.subject ? { subject: options.subject } : {}),
            ...(options.unread === undefined ? {} : { seen: !options.unread }),
            ...(options.since ? { since: options.since } : {}),
            ...(options.before ? { before: options.before } : {}),
          };
          const found = await client.search(criteria, { uid: true });
          if (!Array.isArray(found)) continue;

          // IMAP SEARCH returns ascending sequence order. Read only the newest
          // bounded slice, then merge the folders by parsed received time.
          const uids = found.slice(-perFolderLimit).reverse();
          const mailboxValidity = String(orUndefined(client.mailbox)?.uidValidity);
          for (const uid of uids) {
            const full = orUndefined(
              await client.fetchOne(
                String(uid),
                { source: true, internalDate: true },
                { uid: true },
              ),
            );
            if (!full?.source) continue;
            const parsed = await simpleParser(full.source);
            emails.push(
              mapParsedMessage(
                parsed,
                { folder, uidValidity: mailboxValidity, uid },
                toDate(full.internalDate),
              ),
            );
          }
        } finally {
          lock.release();
        }
      }
    } finally {
      await this.restoreInbox(client);
    }

    return emails
      .sort((a, b) => Date.parse(b.receivedAt) - Date.parse(a.receivedAt))
      .slice(0, options.limit);
  }

  private async findInFolder(
    client: ImapFlow,
    folder: string,
    id: string,
    coords: (MessageCoords & { index?: number }) | undefined,
  ): Promise<FetchedEmail | undefined> {
    const lock = await client.getMailboxLock(folder, { readOnly: true });
    try {
      const mailboxValidity = String(orUndefined(client.mailbox)?.uidValidity);

      let uid: number | undefined;
      if (coords) {
        if (coords.uidValidity !== mailboxValidity) return undefined;
        uid = coords.uid;
      } else {
        const found = await client.search(
          { header: { "message-id": id } },
          { uid: true },
        );
        if (Array.isArray(found) && found.length > 0) uid = found[found.length - 1];
      }
      if (uid === undefined) return undefined;

      const full = orUndefined(
        await client.fetchOne(
          String(uid),
          { source: true, internalDate: true },
          { uid: true },
        ),
      );
      if (!full?.source) return undefined;
      const parsed = await simpleParser(full.source);
      return mapParsedMessage(
        parsed,
        { folder, uidValidity: mailboxValidity, uid },
        toDate(full.internalDate),
      );
    } finally {
      lock.release();
    }
  }

  async downloadAttachment(
    attachment: EmailAttachment,
  ): Promise<DownloadedAttachment | undefined> {
    const target = decodeAttachmentBlobId(attachment.blobId);
    if (!target) {
      this.logger.warn(`Unrecognized attachment handle: ${attachment.blobId}`);
      return undefined;
    }

    const client = this.requireClient();
    try {
      const lock = await client.getMailboxLock(target.folder, { readOnly: true });
      try {
        const mailboxValidity = String(orUndefined(client.mailbox)?.uidValidity);
        if (mailboxValidity !== target.uidValidity) {
          this.logger.warn(
            `Attachment "${attachment.name}" unavailable: ${target.folder} UIDVALIDITY changed`,
          );
          return undefined;
        }

        const full = orUndefined(
          await client.fetchOne(String(target.uid), { source: true }, { uid: true }),
        );
        if (!full?.source) {
          this.logger.warn(
            `Attachment "${attachment.name}" unavailable: message uid=${target.uid} is gone`,
          );
          return undefined;
        }

        const parsed = await simpleParser(full.source);
        const part = parsed.attachments[target.index];
        if (!part) {
          this.logger.warn(
            `Attachment "${attachment.name}" unavailable: part ${target.index} missing`,
          );
          return undefined;
        }
        return {
          name: part.filename ?? attachment.name,
          mimeType: part.contentType,
          data: part.content,
        };
      } finally {
        lock.release();
      }
    } finally {
      await this.restoreInbox(client);
    }
  }

  /** Leave INBOX selected so auto-IDLE watches the right folder at rest. */
  private async restoreInbox(client: ImapFlow): Promise<void> {
    try {
      if (client.usable && orUndefined(client.mailbox)?.path !== "INBOX") {
        await client.mailboxOpen("INBOX", { readOnly: true });
      }
    } catch (error) {
      this.logger.debug(`Failed to reselect INBOX: ${(error as Error).message}`);
    }
  }
}

/** imapflow types several results as `T | false`; normalize to undefined. */
function orUndefined<T>(value: T | false): T | undefined {
  return value === false ? undefined : value;
}

/** imapflow types internalDate as string | Date; normalize to Date. */
function toDate(value: string | Date | undefined): Date | undefined {
  if (value === undefined) return undefined;
  const date = value instanceof Date ? value : new Date(value);
  return Number.isNaN(date.getTime()) ? undefined : date;
}

/** Fallback message ids carry folder coordinates (see mapParsedMessage). */
function decodeMessageId(id: string): MessageCoords | undefined {
  const parts = id.split("|");
  if (parts.length !== 4 || parts[0] !== "imap") return undefined;
  const uid = Number(parts[3]);
  if (!Number.isInteger(uid)) return undefined;
  return { folder: parts[1], uidValidity: parts[2], uid };
}
