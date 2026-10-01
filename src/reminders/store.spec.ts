import {
  mkdtemp,
  readFile,
  readdir,
  rm,
  stat,
  symlink,
  writeFile,
  chmod,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { Effect, Result } from "effect";
import { afterEach, describe, expect, it } from "vitest";
import { createRemindersStore, emptyRemindersState } from "./store.js";

const directories: string[] = [];
async function directory() {
  const location = await mkdtemp(path.join(os.tmpdir(), "omni-reminders-store-test-"));
  directories.push(location);
  return location;
}
afterEach(async () => {
  await Promise.all(
    directories.splice(0).map((dir) => rm(dir, { recursive: true, force: true })),
  );
});

const KEY = "a".repeat(64);
const OTHER_KEY = "b".repeat(64);
const ACCOUNT = "test@example.com";

describe("encrypted Reminders store", () => {
  it("round-trips private state with owner-only file permissions", async () => {
    const dir = await directory();
    const store = createRemindersStore(dir, KEY, ACCOUNT);
    const state = {
      ...emptyRemindersState(),
      session: { sessionToken: "private-token", cookies: "private-cookie" },
      notified: true,
    };
    await Effect.runPromise(store.write(state));
    expect(await Effect.runPromise(store.read())).toEqual(state);
    const [name] = await readdir(dir);
    const bytes = await readFile(path.join(dir, name));
    expect(bytes.toString("utf8")).not.toContain("private-token");
    expect(bytes.toString("utf8")).not.toContain("private-cookie");
    expect((await stat(dir)).mode & 0o777).toBe(0o700);
    expect((await stat(path.join(dir, name))).mode & 0o777).toBe(0o600);
  });

  it("rejects the wrong key and corrupted ciphertext", async () => {
    const dir = await directory();
    const store = createRemindersStore(dir, KEY, ACCOUNT);
    await Effect.runPromise(store.write(emptyRemindersState()));
    const wrong = await Effect.runPromise(
      createRemindersStore(dir, OTHER_KEY, ACCOUNT).read().pipe(Effect.result),
    );
    expect(Result.isFailure(wrong)).toBe(true);
    const [name] = await readdir(dir);
    const file = path.join(dir, name);
    const bytes = await readFile(file);
    bytes[bytes.length - 1] ^= 1;
    await writeFile(file, bytes);
    const corrupt = await Effect.runPromise(store.read().pipe(Effect.result));
    expect(Result.isFailure(corrupt)).toBe(true);
  });

  it("rejects readable-by-others ciphertext", async () => {
    const dir = await directory();
    const store = createRemindersStore(dir, KEY, ACCOUNT);
    await Effect.runPromise(store.write(emptyRemindersState()));
    const [name] = await readdir(dir);
    const file = path.join(dir, name);
    await chmod(file, 0o644);
    expect(
      Result.isFailure(await Effect.runPromise(store.read().pipe(Effect.result))),
    ).toBe(true);
  });

  it("refuses a symlinked private directory", async () => {
    const root = await directory();
    const target = path.join(root, "target");
    const link = path.join(root, "link");
    await symlink(target, link);
    const store = createRemindersStore(link, KEY, ACCOUNT);
    expect(
      Result.isFailure(await Effect.runPromise(store.read().pipe(Effect.result))),
    ).toBe(true);
  });
});
