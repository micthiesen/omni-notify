import fs from "node:fs";
import fsAsync from "node:fs/promises";
import path from "node:path";
import config from "../utils/config.js";

/** Only content-addressed names we generated ourselves are ever served. */
export const AUDIO_FILE_RE = /^[A-Za-z0-9_-]+\.mp3$/;

/** Episode MP3s live next to the SQLite DB so the same volume persists both. */
export function getAudioDir(): string {
  return (
    config.PRESSPODS_AUDIO_DIR ??
    path.join(path.dirname(config.DB_NAME), "press-pods-audio")
  );
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
