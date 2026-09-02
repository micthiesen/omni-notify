import { Entity } from "@micthiesen/mitools/entities";
import { Clock, Effect, Option, Schema } from "effect";

export type ImapFolderCursorData = {
  folder: string;
  /** UIDVALIDITY at the time the cursor was written (BigInt as string). */
  uidValidity: string;
  /** Next unseen UID: everything below this has been dispatched (or skipped). */
  uidNext: number;
  updatedAt: number;
};

const ImapFolderCursorSchema = Schema.Struct({
  folder: Schema.String,
  uidValidity: Schema.String,
  uidNext: Schema.Number,
  updatedAt: Schema.Number,
});

/** Per-folder IMAP delta cursor (iCloud transport only). */
export const ImapFolderCursorEntity = new Entity<ImapFolderCursorData, ["folder"]>(
  "imap-folder-cursor",
  ["folder"],
);

export function getFolderCursorEffect(folder: string) {
  return Effect.gen(function* () {
    const row = yield* ImapFolderCursorEntity.get({ folder });
    return Option.isSome(row)
      ? Schema.decodeUnknownSync(ImapFolderCursorSchema)(row.value)
      : undefined;
  });
}

export function saveFolderCursorEffect(
  folder: string,
  uidValidity: string,
  uidNext: number,
) {
  return Effect.gen(function* () {
    const updatedAt = yield* Clock.currentTimeMillis;
    yield* ImapFolderCursorEntity.upsert({
      folder,
      uidValidity,
      uidNext,
      updatedAt,
    });
  });
}
