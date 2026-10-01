import { createCipheriv, createDecipheriv, createHash, randomBytes } from "node:crypto";
import { constants } from "node:fs";
import { chmod, lstat, mkdir, open, rename, unlink } from "node:fs/promises";
import path from "node:path";
import { Data, Effect, Schema } from "effect";

export class RemindersStoreError extends Data.TaggedError("RemindersStoreError")<{
  readonly operation: "read" | "write";
}> {
  override get message() {
    return "Reminders private storage unavailable";
  }
}

const Operation = Schema.Struct({
  fingerprint: Schema.String,
  recordId: Schema.String,
  state: Schema.Literals(["reserved", "confirmed"]),
  result: Schema.optional(Schema.Unknown),
});
const State = Schema.Struct({
  version: Schema.Literal(1),
  session: Schema.Unknown,
  notified: Schema.Boolean,
  operations: Schema.Record(Schema.String, Operation),
});
export type RemindersStoredState = Schema.Schema.Type<typeof State>;
export const emptyRemindersState = (): RemindersStoredState => ({
  version: 1,
  session: null,
  notified: false,
  operations: {},
});

export interface RemindersStore {
  read(): Effect.Effect<RemindersStoredState, RemindersStoreError>;
  write(state: RemindersStoredState): Effect.Effect<void, RemindersStoreError>;
}

/** Separate from Docstore so generic data browsing/export cannot expose Apple secrets. */
export function createRemindersStore(
  directory: string,
  keyHex: string,
  account: string,
): RemindersStore {
  const key = Buffer.from(keyHex, "hex");
  const identity = createHash("sha256").update(account.toLowerCase()).digest("hex");
  const aad = Buffer.from(`omni-reminders:v1:${identity}`);
  const filename = path.join(directory, `${identity}.enc`);
  const prepare = async () => {
    if (key.length !== 32 || !/^[a-f0-9]{64}$/i.test(keyHex)) throw new Error();
    await mkdir(directory, { recursive: true, mode: 0o700 });
    const stat = await lstat(directory);
    if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error();
    await chmod(directory, 0o700);
  };
  return {
    read: () =>
      Effect.tryPromise({
        try: async () => {
          await prepare();
          let file;
          try {
            file = await open(filename, constants.O_RDONLY | constants.O_NOFOLLOW);
          } catch (error) {
            if (
              typeof error === "object" &&
              error !== null &&
              "code" in error &&
              error.code === "ENOENT"
            )
              return emptyRemindersState();
            throw error;
          }
          try {
            const stat = await file.stat();
            if (
              !stat.isFile() ||
              stat.size > 4 * 1024 * 1024 ||
              (stat.mode & 0o077) !== 0
            )
              throw new Error();
            const bytes = await file.readFile();
            if (bytes.length < 29 || bytes[0] !== 1) throw new Error();
            const decipher = createDecipheriv(
              "aes-256-gcm",
              key,
              bytes.subarray(1, 13),
            );
            decipher.setAAD(aad);
            decipher.setAuthTag(bytes.subarray(13, 29));
            const plain = Buffer.concat([
              decipher.update(bytes.subarray(29)),
              decipher.final(),
            ]);
            return Schema.decodeUnknownSync(State)(JSON.parse(plain.toString("utf8")));
          } finally {
            await file.close();
          }
        },
        catch: () => new RemindersStoreError({ operation: "read" }),
      }),
    write: (state) =>
      Effect.tryPromise({
        try: async () => {
          await prepare();
          const plain = Buffer.from(
            JSON.stringify(Schema.decodeUnknownSync(State)(state)),
          );
          if (plain.length > 4 * 1024 * 1024) throw new Error();
          const iv = randomBytes(12);
          const cipher = createCipheriv("aes-256-gcm", key, iv);
          cipher.setAAD(aad);
          const encrypted = Buffer.concat([cipher.update(plain), cipher.final()]);
          const bytes = Buffer.concat([
            Buffer.from([1]),
            iv,
            cipher.getAuthTag(),
            encrypted,
          ]);
          const temporary = `${filename}.${randomBytes(12).toString("hex")}.tmp`;
          try {
            const file = await open(temporary, "wx", 0o600);
            try {
              await file.writeFile(bytes);
              await file.sync();
            } finally {
              await file.close();
            }
            await rename(temporary, filename);
            const dir = await open(directory, "r");
            try {
              await dir.sync();
            } finally {
              await dir.close();
            }
          } finally {
            await unlink(temporary).catch(() => undefined);
          }
        },
        catch: () => new RemindersStoreError({ operation: "write" }),
      }),
  };
}
