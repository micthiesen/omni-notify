import fs from "node:fs";
import fsAsync from "node:fs/promises";
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
