import { createHash, randomBytes } from "node:crypto";
import fs from "node:fs";
import fsAsync from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { Effect } from "effect";
import config from "../utils/config.js";
import { ignoreFailure, PressPodsError, tryPromise, trySync } from "./effect.js";

/** Only content-addressed names we generated ourselves are ever served. */
export const AUDIO_FILE_RE = /^[A-Za-z0-9_-]+\.mp3$/;

/** Episode MP3s live next to the SQLite DB so the same volume persists both. */
export function getAudioDir(): string {
  if (config.PRESSPODS_AUDIO_DIR) return config.PRESSPODS_AUDIO_DIR;
  // Mirror mitools' docstore resolution: DB_NAME may be a bare file name, and
  // in Docker the DB lives under /data regardless.
  const dbPath = config.DOCKERIZED ? `/data/${config.DB_NAME}` : config.DB_NAME;
  return path.join(path.dirname(dbPath), "press-pods-audio");
}

export const ensureAudioDir = (): Effect.Effect<void, PressPodsError> =>
  trySync("create PressPods audio directory", () => {
    fs.mkdirSync(getAudioDir(), { recursive: true });
  });

export function episodeAudioPath(fileName: string): string {
  if (!AUDIO_FILE_RE.test(fileName)) {
    throw new Error(`Invalid episode audio file name: ${fileName}`);
  }
  return path.join(getAudioDir(), fileName);
}

export const saveEpisodeAudio = (
  fileName: string,
  audio: Buffer,
): Effect.Effect<void, PressPodsError> =>
  Effect.gen(function* () {
    yield* ensureAudioDir();
    yield* tryPromise("write episode audio", (signal) =>
      fsAsync.writeFile(episodeAudioPath(fileName), audio, { signal }),
    );
  });

/**
 * Best-effort delete of an episode's audio file. Never throws: the DB row is the
 * source of truth, and a leftover (or already-gone) file is harmless — an
 * unreferenced, unguessably-named MP3. Callers delete the row first, so this
 * failing must not turn a successful delete/replace into an error.
 */
export const deleteEpisodeAudio = (fileName: string): Effect.Effect<void> =>
  AUDIO_FILE_RE.test(fileName)
    ? ignoreFailure(
        tryPromise("delete episode audio", () =>
          fsAsync.rm(episodeAudioPath(fileName), { force: true }),
        ),
      )
    : Effect.void;

// ---------------------------------------------------------------------------
// Per-chunk synthesis checkpoints (restart resilience). Each verified chunk's
// prepared WAV is cached on disk keyed by (article identity, render signature,
// chunk text) so a process killed mid-synthesis resumes from the last good
// chunk instead of re-synthesizing (and, on ElevenLabs, re-paying for) every
// chunk. The cache lives under the audio volume so it survives restarts, and
// is scoped per-article so completing an episode can drop the whole set.
// The cache is strictly an optimization: every read/write is best-effort and
// falls through to normal synthesis on any error, so a bad checkpoint can
// never corrupt an episode.
// ---------------------------------------------------------------------------

/** Stable, filesystem-safe id for an article's checkpoint set, from its
 * canonical (normalized) URL. */
export function checkpointWorkId(normalizedUrl: string): string {
  return createHash("sha256").update(normalizedUrl).digest("hex").slice(0, 16);
}

function checkpointDir(workId: string): string {
  return path.join(getAudioDir(), ".chunks", workId);
}

/** Content-addressed key for one prepared chunk: render signature + text. The
 * `\0` separator (a NUL, kept as a source escape so this file stays plain text)
 * makes the boundary unambiguous — no signature+text pair can collide with a
 * different split of the same concatenation. */
export function checkpointKey(signature: string, text: string): string {
  return `${createHash("sha256").update(`${signature}\0${text}`).digest("hex")}.wav`;
}

/** Cached prepared WAV bytes for a chunk, or null on miss / any read error. */
export function readChunkCheckpoint(
  workId: string,
  key: string,
): Effect.Effect<Buffer | null> {
  return tryPromise("read chunk checkpoint", (signal) =>
    fsAsync.readFile(path.join(checkpointDir(workId), key), { signal }),
  ).pipe(Effect.catch(() => Effect.succeed(null)));
}

/** Atomically cache a prepared chunk WAV (temp file + rename so a kill
 * mid-write can never leave a truncated file that reads as a valid take). */
export function writeChunkCheckpoint(
  workId: string,
  key: string,
  wav: Buffer,
): Effect.Effect<void> {
  return Effect.gen(function* () {
    const dir = checkpointDir(workId);
    yield* tryPromise("create checkpoint directory", () =>
      fsAsync.mkdir(dir, { recursive: true }),
    );
    const tmp = path.join(dir, `.tmp-${randomBytes(8).toString("hex")}`);
    yield* tryPromise("write chunk checkpoint", (signal) =>
      fsAsync.writeFile(tmp, wav, { signal }),
    ).pipe(
      Effect.flatMap(() =>
        tryPromise("commit chunk checkpoint", () =>
          fsAsync.rename(tmp, path.join(dir, key)),
        ),
      ),
      Effect.ensuring(
        ignoreFailure(
          tryPromise("remove checkpoint temporary file", () =>
            fsAsync.rm(tmp, { force: true }),
          ),
        ),
      ),
    );
  }).pipe(Effect.catch(() => Effect.void));
}

/** Drop a single (e.g. corrupt) checkpoint file so it isn't retried forever. */
export function deleteChunkCheckpoint(
  workId: string,
  key: string,
): Effect.Effect<void> {
  return ignoreFailure(
    tryPromise("delete chunk checkpoint", () =>
      fsAsync.rm(path.join(checkpointDir(workId), key), { force: true }),
    ),
  );
}

/** Drop an article's whole checkpoint set (episode finished or abandoned). */
export const clearChunkCheckpoints = (workId: string): Effect.Effect<void> =>
  ignoreFailure(
    tryPromise("clear chunk checkpoints", () =>
      fsAsync.rm(checkpointDir(workId), { recursive: true, force: true }),
    ),
  );

/** Materialize cached WAV bytes to a fresh temp file so the rest of the
 * pipeline treats it exactly like a freshly-prepared chunk (and cleanupWavs
 * removes it normally). */
export function materializeCheckpointWav(
  wav: Buffer,
): Effect.Effect<string, PressPodsError> {
  const tmp = path.join(
    os.tmpdir(),
    `presspods-ckpt-${randomBytes(12).toString("hex")}.wav`,
  );
  return tryPromise("materialize chunk checkpoint", (signal) =>
    fsAsync.writeFile(tmp, wav, { signal }),
  ).pipe(Effect.as(tmp));
}
