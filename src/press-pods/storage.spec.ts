import fsAsync from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { Injector } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

// getAudioDir() derives from DB_NAME's directory when PRESSPODS_AUDIO_DIR is
// unset, so point DB_NAME at an isolated temp dir and the audio (checkpoint)
// dir lands beside it.
const TMP_ROOT = path.join(os.tmpdir(), "presspods-storage-spec");

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.ERROR,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: path.join(TMP_ROOT, "storage.spec.db"),
  },
});

const {
  checkpointKey,
  checkpointWorkId,
  clearChunkCheckpoints,
  deleteChunkCheckpoint,
  deleteEpisodeAudio,
  materializeCheckpointWav,
  readChunkCheckpoint,
  writeChunkCheckpoint,
} = await import("./storage.js");

beforeAll(async () => {
  await fsAsync.rm(TMP_ROOT, { recursive: true, force: true });
});
afterAll(async () => {
  await fsAsync.rm(TMP_ROOT, { recursive: true, force: true });
});

describe("checkpointWorkId", () => {
  it("is deterministic and filesystem-safe", () => {
    const id = checkpointWorkId("https://example.com/a");
    expect(id).toBe(checkpointWorkId("https://example.com/a"));
    expect(id).toMatch(/^[a-f0-9]{16}$/);
  });

  it("differs for different URLs", () => {
    expect(checkpointWorkId("https://example.com/a")).not.toBe(
      checkpointWorkId("https://example.com/b"),
    );
  });
});

describe("checkpointKey", () => {
  it("is deterministic for the same signature + text", () => {
    expect(checkpointKey("sig", "hello world")).toBe(
      checkpointKey("sig", "hello world"),
    );
  });

  it("changes with the render signature (voice/provider/speed change)", () => {
    expect(checkpointKey("sigA", "text")).not.toBe(checkpointKey("sigB", "text"));
  });

  it("has an unambiguous signature/text boundary (no concatenation collision)", () => {
    // Without a delimiter, ("sig","text") and ("si","gtext") would collide.
    expect(checkpointKey("sig", "text")).not.toBe(checkpointKey("si", "gtext"));
  });
});

describe("chunk checkpoint round-trip", () => {
  const workId = "workid-roundtrip";

  it("returns null on a miss", async () => {
    expect(await readChunkCheckpoint(workId, checkpointKey("s", "missing"))).toBeNull();
  });

  it("writes then reads back the exact bytes", async () => {
    const key = checkpointKey("s", "chunk one");
    const wav = Buffer.from("fake-wav-bytes");
    await writeChunkCheckpoint(workId, key, wav);
    const read = await readChunkCheckpoint(workId, key);
    expect(read?.equals(wav)).toBe(true);
  });

  it("deletes a single checkpoint", async () => {
    const key = checkpointKey("s", "chunk two");
    await writeChunkCheckpoint(workId, key, Buffer.from("x"));
    await deleteChunkCheckpoint(workId, key);
    expect(await readChunkCheckpoint(workId, key)).toBeNull();
  });

  it("clears the whole work set", async () => {
    const key = checkpointKey("s", "chunk three");
    await writeChunkCheckpoint(workId, key, Buffer.from("y"));
    await clearChunkCheckpoints(workId);
    expect(await readChunkCheckpoint(workId, key)).toBeNull();
  });

  it("clearing a non-existent work set is a no-op (never throws)", async () => {
    await expect(clearChunkCheckpoints("never-created")).resolves.toBeUndefined();
  });
});

describe("materializeCheckpointWav", () => {
  it("writes the bytes to a fresh readable temp file", async () => {
    const wav = Buffer.from("materialized");
    const p = await materializeCheckpointWav(wav);
    expect((await fsAsync.readFile(p)).equals(wav)).toBe(true);
    await fsAsync.rm(p, { force: true });
  });
});

describe("deleteEpisodeAudio", () => {
  it("never throws for a missing file", async () => {
    await expect(deleteEpisodeAudio("does-not-exist.mp3")).resolves.toBeUndefined();
  });

  it("ignores names that aren't valid audio files", async () => {
    await expect(deleteEpisodeAudio("../escape")).resolves.toBeUndefined();
  });
});
