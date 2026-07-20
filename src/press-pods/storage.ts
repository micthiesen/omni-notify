import { createHash, randomBytes } from "node:crypto";
import fs from "node:fs";
import fsAsync from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import config from "../utils/config.js";

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

export function ensureAudioDir(): void {
  fs.mkdirSync(getAudioDir(), { recursive: true });
}

export function episodeAudioPath(fileName: string): string {
  if (!AUDIO_FILE_RE.test(fileName)) {
    throw new Error(`Invalid episode audio file name: ${fileName}`);
  }
  return path.join(getAudioDir(), fileName);
}

export async function saveEpisodeAudio(fileName: string, audio: Buffer): Promise<void> {
  ensureAudioDir();
  await fsAsync.writeFile(episodeAudioPath(fileName), audio);
}

/**
 * Best-effort delete of an episode's audio file. Never throws: the DB row is the
 * source of truth, and a leftover (or already-gone) file is harmless — an
 * unreferenced, unguessably-named MP3. Callers delete the row first, so this
 * failing must not turn a successful delete/replace into an error.
 */
export async function deleteEpisodeAudio(fileName: string): Promise<void> {
  if (!AUDIO_FILE_RE.test(fileName)) return;
  try {
    await fsAsync.rm(episodeAudioPath(fileName), { force: true });
  } catch {
    // ignore — the row is already gone; a stray file is not worth failing on.
  }
}

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
export async function readChunkCheckpoint(
  workId: string,
  key: string,
): Promise<Buffer | null> {
  try {
    return await fsAsync.readFile(path.join(checkpointDir(workId), key));
  } catch {
    return null;
  }
}

/** Atomically cache a prepared chunk WAV (temp file + rename so a kill
 * mid-write can never leave a truncated file that reads as a valid take). */
export async function writeChunkCheckpoint(
  workId: string,
  key: string,
  wav: Buffer,
): Promise<void> {
  try {
    const dir = checkpointDir(workId);
    await fsAsync.mkdir(dir, { recursive: true });
    const tmp = path.join(dir, `.tmp-${randomBytes(8).toString("hex")}`);
    await fsAsync.writeFile(tmp, wav);
    await fsAsync.rename(tmp, path.join(dir, key));
  } catch {
    // Best-effort: a failed checkpoint just means no resume speedup.
  }
}

/** Drop a single (e.g. corrupt) checkpoint file so it isn't retried forever. */
export async function deleteChunkCheckpoint(
  workId: string,
  key: string,
): Promise<void> {
  try {
    await fsAsync.rm(path.join(checkpointDir(workId), key), { force: true });
  } catch {
    // Best-effort.
  }
}

/** Drop an article's whole checkpoint set (episode finished or abandoned). */
export async function clearChunkCheckpoints(workId: string): Promise<void> {
  try {
    await fsAsync.rm(checkpointDir(workId), { recursive: true, force: true });
  } catch {
    // Best-effort cleanup.
  }
}

/** Materialize cached WAV bytes to a fresh temp file so the rest of the
 * pipeline treats it exactly like a freshly-prepared chunk (and cleanupWavs
 * removes it normally). */
export async function materializeCheckpointWav(wav: Buffer): Promise<string> {
  const tmp = path.join(
    os.tmpdir(),
    `presspods-ckpt-${randomBytes(12).toString("hex")}.wav`,
  );
  await fsAsync.writeFile(tmp, wav);
  return tmp;
}
