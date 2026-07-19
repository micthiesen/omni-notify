import { execFile } from "node:child_process";
import { randomBytes } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

/**
 * Audio assembly for PressPods episodes. Raw fluent-ffmpeg can't express the
 * multi-stage filtergraph this needs (per-chunk leveling, two-pass linear
 * loudnorm, click-free joins), so this drives ffmpeg directly. ffmpeg must be
 * on PATH (installed in the Docker runtime image).
 */

const SAMPLE_RATE = 44100;
/** Per-chunk leveling target; the final master lifts everything to -16 LUFS. */
const CHUNK_LUFS = -19;
/** Delivery target: -16 LUFS / -1.5 dBTP is the podcast convention. */
const MASTER_LUFS = -16;
/** Short fades at each chunk edge so butt-joins don't click. */
const EDGE_FADE_SEC = 0.012;

async function ffmpeg(args: string[]): Promise<string> {
  const { stderr } = await execFileAsync("ffmpeg", ["-hide_banner", "-y", ...args], {
    maxBuffer: 128 * 1024 * 1024,
  });
  return stderr;
}

function tmpFile(ext: string): string {
  return path.join(os.tmpdir(), `pp_${randomBytes(8).toString("hex")}.${ext}`);
}

export async function probeDurationSeconds(file: string): Promise<number> {
  const { stdout } = await execFileAsync("ffprobe", [
    "-v",
    "error",
    "-show_entries",
    "format=duration",
    "-of",
    "csv=p=0",
    file,
  ]);
  return Number.parseFloat(stdout.trim());
}

/**
 * Two-pass linear loudnorm. Pass 1 measures; pass 2 applies a single linear
 * gain (transparent — no dynamic pumping). One-pass loudnorm runs a dynamic
 * AGC that pumps and lifts quiet passages, so it is deliberately not used.
 */
async function twoPassLoudnorm(
  inFile: string,
  outFile: string,
  target: number,
  toWav: boolean,
): Promise<void> {
  const spec = `I=${target}:TP=-1.5:LRA=11`;
  const stderr = await ffmpeg([
    "-i",
    inFile,
    "-af",
    `loudnorm=${spec}:print_format=json`,
    "-f",
    "null",
    "-",
  ]);
  const jsonMatch = stderr.match(/\{[^{}]*"input_i"[\s\S]*?\}/);
  if (!jsonMatch) throw new Error(`loudnorm measurement failed for ${inFile}`);
  const m = JSON.parse(jsonMatch[0]);
  const filter =
    `loudnorm=${spec}:linear=true` +
    `:measured_I=${m.input_i}:measured_TP=${m.input_tp}` +
    `:measured_LRA=${m.input_lra}:measured_thresh=${m.input_thresh}` +
    `:offset=${m.target_offset},aresample=${SAMPLE_RATE}`;
  const encode = toWav ? ["-c:a", "pcm_s16le"] : ["-c:a", "libmp3lame", "-b:a", "96k"];
  await ffmpeg([
    "-i",
    inFile,
    "-af",
    filter,
    "-ar",
    String(SAMPLE_RATE),
    "-ac",
    "1",
    ...encode,
    outFile,
  ]);
}

/**
 * Turn one raw TTS chunk (MP3 bytes) into a concat-ready WAV: trim edge
 * silence, apply short edge fades, and level to a fixed per-chunk LUFS so no
 * chunk sits quieter than its neighbors. Returns the WAV path + its duration
 * (used to compute chapter offsets). The `areverse` sandwich trims + fades the
 * trailing edge without needing to know the duration up front.
 */
export async function prepareChunk(
  mp3: Buffer,
): Promise<{ wavPath: string; durationSeconds: number }> {
  const rawPath = tmpFile("mp3");
  const trimmedPath = tmpFile("wav");
  const wavPath = tmpFile("wav");
  await fs.writeFile(rawPath, mp3);
  try {
    const edge =
      "silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.15," +
      `afade=t=in:st=0:d=${EDGE_FADE_SEC},` +
      "areverse," +
      "silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.25," +
      `afade=t=in:st=0:d=${EDGE_FADE_SEC},` +
      "areverse";
    await ffmpeg([
      "-i",
      rawPath,
      "-af",
      edge,
      "-ar",
      String(SAMPLE_RATE),
      "-ac",
      "1",
      trimmedPath,
    ]);
    await twoPassLoudnorm(trimmedPath, wavPath, CHUNK_LUFS, true);
    const durationSeconds = await probeDurationSeconds(wavPath);
    return { wavPath, durationSeconds };
  } finally {
    await fs.unlink(rawPath).catch(() => {});
    await fs.unlink(trimmedPath).catch(() => {});
  }
}

/** A silence WAV of the given length, used as a gap between chunks/sections. */
export async function makeSilenceWav(seconds: number): Promise<string> {
  const out = tmpFile("wav");
  await ffmpeg([
    "-f",
    "lavfi",
    "-i",
    `anullsrc=r=${SAMPLE_RATE}:cl=mono`,
    "-t",
    seconds.toFixed(3),
    out,
  ]);
  return out;
}

/** Concatenate same-format WAVs (concat demuxer, stream copy). */
async function concatWavs(files: string[]): Promise<string> {
  const listPath = tmpFile("txt");
  const out = tmpFile("wav");
  await fs.writeFile(
    listPath,
    files.map((f) => `file '${f.replace(/'/g, "'\\''")}'`).join("\n"),
  );
  try {
    await ffmpeg(["-f", "concat", "-safe", "0", "-i", listPath, "-c", "copy", out]);
    return out;
  } finally {
    await fs.unlink(listPath).catch(() => {});
  }
}

/**
 * Assemble the episode: concat the prepared chunk/gap WAVs, master to -16 LUFS
 * (two-pass linear), then prepend the intro jingle and encode once to MP3
 * (96k mono — transparent for speech, half the size of 128k). A single final
 * encode replaces the old three-generation MP3 chain.
 */
export async function assembleEpisode(
  chunkWavPaths: string[],
  introMp3: Buffer,
): Promise<Buffer> {
  const speechRaw = await concatWavs(chunkWavPaths);
  const speechMastered = tmpFile("wav");
  const introPath = tmpFile("mp3");
  const outPath = tmpFile("mp3");
  try {
    await twoPassLoudnorm(speechRaw, speechMastered, MASTER_LUFS, true);
    await fs.writeFile(introPath, introMp3);
    // Conform both inputs to 44.1k mono, loudness-match the intro to the same
    // target, then concat and encode once. filter_complex handles the join
    // click-free without the codec-padding gaps of an MP3-level concat.
    await ffmpeg([
      "-i",
      introPath,
      "-i",
      speechMastered,
      "-filter_complex",
      `[0:a]aresample=${SAMPLE_RATE},aformat=channel_layouts=mono,` +
        `loudnorm=I=${MASTER_LUFS}:TP=-1.5:LRA=11[intro];` +
        `[1:a]aresample=${SAMPLE_RATE},aformat=channel_layouts=mono[speech];` +
        `[intro][speech]concat=n=2:v=0:a=1[out]`,
      "-map",
      "[out]",
      "-ar",
      String(SAMPLE_RATE),
      "-ac",
      "1",
      "-c:a",
      "libmp3lame",
      "-b:a",
      "96k",
      "-write_xing",
      "1",
      outPath,
    ]);
    return await fs.readFile(outPath);
  } finally {
    for (const f of [speechRaw, speechMastered, introPath, outPath]) {
      await fs.unlink(f).catch(() => {});
    }
  }
}

/** Best-effort cleanup of prepared chunk/gap WAVs after assembly. */
export async function cleanupWavs(files: Iterable<string>): Promise<void> {
  for (const f of new Set(files)) await fs.unlink(f).catch(() => {});
}
