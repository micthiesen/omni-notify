/**
 * Pure IMAP folder-sync planning. New-mail detection is UID-based: everything
 * at or above the cursor's uidNext is new. (CONDSTORE/QRESYNC would add flag
 * -change deltas, which this pipeline doesn't consume — and iCloud rejects
 * parameterized `SELECT ... (CONDSTORE)` anyway, see MailKit #970.)
 */

export interface FolderCursorState {
  uidValidity: string;
  uidNext: number;
}

export interface FolderStatusState {
  uidValidity: string;
  uidNext: number;
}

export type FolderSyncPlan =
  /** First contact with this folder: record the cursor, skip history. */
  | { action: "init" }
  /** Nothing new since the cursor. */
  | { action: "none" }
  /** Fetch UIDs >= fromUid. */
  | { action: "fetch"; fromUid: number }
  /** UIDVALIDITY changed: UIDs are meaningless, recover by received-date. */
  | { action: "reset" };

export function planFolderSync(
  cursor: FolderCursorState | undefined,
  status: FolderStatusState,
): FolderSyncPlan {
  if (!cursor) return { action: "init" };
  if (cursor.uidValidity !== status.uidValidity) return { action: "reset" };
  if (status.uidNext > cursor.uidNext)
    return { action: "fetch", fromUid: cursor.uidNext };
  return { action: "none" };
}
