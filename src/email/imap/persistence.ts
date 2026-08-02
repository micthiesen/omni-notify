import { Entity } from "@micthiesen/mitools/entities";

export type ImapFolderCursorData = {
  folder: string;
  /** UIDVALIDITY at the time the cursor was written (BigInt as string). */
  uidValidity: string;
  /** Next unseen UID: everything below this has been dispatched (or skipped). */
  uidNext: number;
  updatedAt: number;
};

/** Per-folder IMAP delta cursor (iCloud transport only). */
export const ImapFolderCursorEntity = new Entity<ImapFolderCursorData, ["folder"]>(
  "imap-folder-cursor",
  ["folder"],
);

export function getFolderCursor(folder: string): ImapFolderCursorData | undefined {
  return ImapFolderCursorEntity.get({ folder });
}

export function saveFolderCursor(
  folder: string,
  uidValidity: string,
  uidNext: number,
): void {
  ImapFolderCursorEntity.upsert({
    folder,
    uidValidity,
    uidNext,
    updatedAt: Date.now(),
  });
}
