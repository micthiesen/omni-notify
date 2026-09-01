import { execFile } from "node:child_process";
import { randomBytes } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { Effect, Exit, Schema } from "effect";
import {
  ignoreFailure,
  InvalidPressPodsDataError,
  PressPodsError,
  tryPromise,
} from "../effect.js";

const execFileAsync = promisify(execFile);

/**
 * Audio assembly for PressPods episodes. Raw fluent-ffmpeg can't express the
 * multi-stage filtergraph this needs (per-chunk leveling, two-pass linear
 * loudnorm, click-free joins), so this drives ffmpeg directly. ffmpeg must be
 * on PATH (installed in the Docker runtime image).
 */

const SAMPLE_RATE = 44100;
/**
 * Playback-speed multiplier applied to narration (not the intro jingle) via a
 * pitch-preserving `atempo` time-stretch — the cleanest way to nudge pace
 * without touching the model's own unreliable speed handling. 1.1 = +10%.
 * Applied first in prepareChunk so the returned duration (and every chapter /
 * chunk offset derived from it) already reflects the sped audio. Kept well
 * inside atempo's transparent 0.5–2.0 range.
 */
export const SPEED_MULTIPLIER = 1.1;
/** Per-chunk leveling target; the final master lifts everything to -16 LUFS. */
const CHUNK_LUFS = -19;
/** Delivery target: -16 LUFS / -1.5 dBTP is the podcast convention. */
const MASTER_LUFS = -16;
/** Short fades at each chunk edge so butt-joins don't click. */
const EDGE_FADE_SEC = 0.012;
/**
 * swresample's default anti-imaging filter is weak: upsampling Higgs's 24kHz
 * output with defaults mirrors the vocoder's band-edge energy (~10.7kHz)
 * around 12kHz into an audible ~13kHz "ring" that shadows every sibilant.
 * These params bury the images below the noise floor (measured: -95dB →
 * -136dB in sibilant frames). Every aresample in this file must carry them,
 * including ones that pre-empt auto-inserted conversions (arnndn forces 48k,
 * loudnorm runs a 192k round-trip internally). See docs/presspods-audio.md.
 */
const RESAMPLE_HQ = "filter_size=256:cutoff=0.95";
/**
 * Speech denoise for self-hosted models with a noise floor (e.g. Higgs): a
 * sub-80Hz rumble cut plus RNNoise (`arnndn`). RNNoise is used over `afftdn`
 * because spectral subtraction leaves a metallic/"musical noise" tang on voice;
 * the RNN model suppresses the noise floor cleanly. Model ships in assets.
 */
const DENOISE_MODEL_PATH = "assets/press-pods/denoise.rnnn";
/**
 * Higgs's vocoder synthesizes sibilance as a dense comb of narrowband peaks
 * running right up to its ~11kHz band edge. Below ~9.5kHz the comb is masked
 * by the sibilance it rides on; the 9.8-11kHz remainder pokes above the
 * sibilance rolloff and reads as a faint metallic rattle shadowing every "s".
 * This steep linear-phase FIR shelf removes that exposed octave (measured:
 * -62dB → -88dB in sibilant frames) while leaving ≤9.5kHz untouched. The
 * in-band comb below 9.5kHz is a model artifact post-processing can't remove
 * without dulling sibilance; see docs/presspods-audio.md.
 */
const FIZZ_SHELF =
  "firequalizer=gain='if(lt(f,9600),0,if(gt(f,10300),-30,-30*(f-9600)/700))'";
/** The explicit HQ aresample pre-empts the default-quality one ffmpeg would
 * auto-insert for arnndn (RNNoise only runs at 48kHz). */
const DENOISE_FILTER =
  `highpass=f=80,aresample=48000:${RESAMPLE_HQ},` +
  `arnndn=m=${DENOISE_MODEL_PATH},${FIZZ_SHELF}`;

function ffmpeg(args: string[]): Effect.Effect<string, PressPodsError> {
  return tryPromise("run ffmpeg", (signal) =>
    execFileAsync("ffmpeg", ["-hide_banner", "-y", ...args], {
      maxBuffer: 128 * 1024 * 1024,
      signal,
    }),
  ).pipe(Effect.map(({ stderr }) => stderr));
}

function tmpFile(ext: string): string {
  return path.join(os.tmpdir(), `pp_${randomBytes(8).toString("hex")}.${ext}`);
}

const DurationOutputSchema = Schema.NumberFromString.check(
  Schema.makeFilter((duration) => Number.isFinite(duration) && duration >= 0),
);

export function probeDurationSeconds(
  file: string,
): Effect.Effect<number, PressPodsError | InvalidPressPodsDataError> {
  return tryPromise("probe audio duration", (signal) =>
    execFileAsync(
      "ffprobe",
      ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", file],
      { signal },
    ),
  ).pipe(
    Effect.flatMap(({ stdout }) =>
      Schema.decodeUnknownEffect(DurationOutputSchema)(stdout.trim()).pipe(
        Effect.mapError(
          (cause) =>
            new InvalidPressPodsDataError({
              operation: "decode ffprobe duration",
              cause,
            }),
        ),
      ),
    ),
  );
}

/**
 * Two-pass linear loudnorm. Pass 1 measures; pass 2 applies a single linear
 * gain (transparent — no dynamic pumping). One-pass loudnorm runs a dynamic
 * AGC that pumps and lifts quiet passages, so it is deliberately not used.
 */
function twoPassLoudnorm(
  inFile: string,
  outFile: string,
  target: number,
  toWav: boolean,
): Effect.Effect<void, PressPodsError | InvalidPressPodsDataError> {
  return Effect.gen(function* () {
    const spec = `I=${target}:TP=-1.5:LRA=11`;
    const stderr = yield* ffmpeg([
      "-i",
      inFile,
      "-af",
      `loudnorm=${spec}:print_format=json`,
      "-f",
      "null",
      "-",
    ]);
    const jsonMatch = stderr.match(/\{[^{}]*"input_i"[\s\S]*?\}/);
    if (!jsonMatch) {
      return yield* new InvalidPressPodsDataError({
        operation: "decode ffmpeg loudnorm measurement",
        cause: new Error(`loudnorm measurement failed for ${inFile}`),
      });
    }
    const LoudnormSchema = Schema.Struct({
      input_i: Schema.String,
      input_tp: Schema.String,
      input_lra: Schema.String,
      input_thresh: Schema.String,
      target_offset: Schema.String,
    });
    const m = yield* Effect.try({
      try: () => JSON.parse(jsonMatch[0]) as unknown,
      catch: (cause) =>
        new InvalidPressPodsDataError({ operation: "parse loudnorm JSON", cause }),
    }).pipe(
      Effect.flatMap(Schema.decodeUnknownEffect(LoudnormSchema)),
      Effect.mapError((cause) =>
        cause instanceof InvalidPressPodsDataError
          ? cause
          : new InvalidPressPodsDataError({ operation: "decode loudnorm JSON", cause }),
      ),
    );
    const filter =
      `loudnorm=${spec}:linear=true` +
      `:measured_I=${m.input_i}:measured_TP=${m.input_tp}` +
      `:measured_LRA=${m.input_lra}:measured_thresh=${m.input_thresh}` +
      `:offset=${m.target_offset},aresample=${SAMPLE_RATE}:${RESAMPLE_HQ}`;
    const encode = toWav
      ? ["-c:a", "pcm_s16le"]
      : ["-c:a", "libmp3lame", "-b:a", "96k"];
    yield* ffmpeg([
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
  });
}

/**
 * Turn one raw TTS chunk (MP3 bytes) into a concat-ready WAV: optionally
 * denoise, trim edge silence, apply short edge fades, and level to a fixed
 * per-chunk LUFS so no chunk sits quieter than its neighbors. Returns the WAV
 * path + its duration (used to compute chapter offsets). The `areverse`
 * sandwich trims + fades the trailing edge without needing the duration up
 * front; denoise runs first so leveling doesn't amplify the noise floor.
 */
export interface PreparedChunk {
  wavPath: string;
  durationSeconds: number;
}

export function prepareChunk(
  mp3: Buffer,
  { denoise = false }: { denoise?: boolean } = {},
): Effect.Effect<PreparedChunk, PressPodsError | InvalidPressPodsDataError> {
  const wavPath = tmpFile("wav");
  return withTemporaryPath("mp3", (rawPath) =>
    withTemporaryPath("wav", (trimmedPath) =>
      Effect.gen(function* () {
        yield* tryPromise("write raw TTS chunk", (signal) =>
          fs.writeFile(rawPath, mp3, { signal }),
        );
        const edge =
          `atempo=${SPEED_MULTIPLIER},` +
          (denoise ? `${DENOISE_FILTER},` : "") +
          "silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.15," +
          `afade=t=in:st=0:d=${EDGE_FADE_SEC},` +
          "areverse," +
          "silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.25," +
          `afade=t=in:st=0:d=${EDGE_FADE_SEC},` +
          "areverse," +
          `aresample=${SAMPLE_RATE}:${RESAMPLE_HQ}`;
        yield* ffmpeg([
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
        yield* twoPassLoudnorm(trimmedPath, wavPath, CHUNK_LUFS, true);
        const durationSeconds = yield* probeDurationSeconds(wavPath);
        return { wavPath, durationSeconds };
      }),
    ),
  ).pipe(
    Effect.onExit((exit) => (Exit.isFailure(exit) ? removeFile(wavPath) : Effect.void)),
  );
}

/** A silence WAV of the given length, used as a gap between chunks/sections. */
export function makeSilenceWav(seconds: number): Effect.Effect<string, PressPodsError> {
  const out = tmpFile("wav");
  return ffmpeg([
    "-f",
    "lavfi",
    "-i",
    `anullsrc=r=${SAMPLE_RATE}:cl=mono`,
    "-t",
    seconds.toFixed(3),
    out,
  ]).pipe(
    Effect.as(out),
    Effect.onError(() => removeFile(out)),
  );
}

/** Concatenate same-format WAVs (concat demuxer, stream copy). */
function concatWavs(files: string[]): Effect.Effect<string, PressPodsError> {
  const out = tmpFile("wav");
  return withTemporaryPath("txt", (listPath) =>
    Effect.gen(function* () {
      yield* tryPromise("write ffmpeg concat manifest", (signal) =>
        fs.writeFile(
          listPath,
          files.map((f) => `file '${f.replace(/'/g, "'\\''")}'`).join("\n"),
          { signal },
        ),
      );
      yield* ffmpeg(["-f", "concat", "-safe", "0", "-i", listPath, "-c", "copy", out]);
      return out;
    }),
  ).pipe(
    Effect.onExit((exit) => (Exit.isFailure(exit) ? removeFile(out) : Effect.void)),
  );
}

/**
 * Assemble the episode: concat the prepared chunk/gap WAVs, master to -16 LUFS
 * (two-pass linear), then prepend the intro jingle and encode once to MP3
 * (96k mono — transparent for speech, half the size of 128k). A single final
 * encode replaces the old three-generation MP3 chain.
 */
export function assembleEpisode(
  chunkWavPaths: string[],
  introMp3: Buffer,
): Effect.Effect<Buffer, PressPodsError | InvalidPressPodsDataError> {
  let speechRaw: string | undefined;
  const speechMastered = tmpFile("wav");
  const introPath = tmpFile("mp3");
  const outPath = tmpFile("mp3");
  return Effect.gen(function* () {
    speechRaw = yield* concatWavs(chunkWavPaths);
    yield* twoPassLoudnorm(speechRaw, speechMastered, MASTER_LUFS, true);
    yield* tryPromise("write intro audio", (signal) =>
      fs.writeFile(introPath, introMp3, { signal }),
    );
    // Conform both inputs to 44.1k mono, loudness-match the intro to the same
    // target, then concat and encode once. filter_complex handles the join
    // click-free without the codec-padding gaps of an MP3-level concat.
    yield* ffmpeg([
      "-i",
      introPath,
      "-i",
      speechMastered,
      "-filter_complex",
      `[0:a]aresample=${SAMPLE_RATE}:${RESAMPLE_HQ},aformat=channel_layouts=mono,` +
        `loudnorm=I=${MASTER_LUFS}:TP=-1.5:LRA=11,` +
        `aresample=${SAMPLE_RATE}:${RESAMPLE_HQ}[intro];` +
        `[1:a]aresample=${SAMPLE_RATE}:${RESAMPLE_HQ},aformat=channel_layouts=mono[speech];` +
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
    return yield* tryPromise("read assembled episode", (signal) =>
      fs.readFile(outPath, { signal }),
    );
  }).pipe(
    Effect.ensuring(
      Effect.forEach(
        [speechRaw, speechMastered, introPath, outPath].filter(
          (file): file is string => file !== undefined,
        ),
        removeFile,
        { discard: true },
      ),
    ),
  );
}

/** Best-effort cleanup of prepared chunk/gap WAVs after assembly. */
function removeFile(file: string): Effect.Effect<void> {
  return ignoreFailure(
    tryPromise("remove temporary audio file", () => fs.unlink(file)),
  );
}

function withTemporaryPath<A, E>(
  extension: string,
  use: (file: string) => Effect.Effect<A, E>,
): Effect.Effect<A, E> {
  return Effect.scoped(
    Effect.acquireRelease(
      Effect.sync(() => tmpFile(extension)),
      removeFile,
    ).pipe(Effect.flatMap(use)),
  );
}

export function cleanupWavs(files: Iterable<string>): Effect.Effect<void> {
  return Effect.forEach(new Set(files), removeFile, { discard: true });
}
