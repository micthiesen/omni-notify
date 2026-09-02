/**
 * TTS bake-off: run one or more articles through the real PressPods retrieval +
 * narration-cleaning pipeline, then synthesize the SAME narration text with
 * multiple TTS providers for ears-on comparison.
 *
 * Throwaway integration script — not part of the app. Run with:
 *
 *   npx dotenvx run -- bun src/tools/tts-bakeoff.ts <article-url> [...more-urls]
 *
 * Flags:
 *   --providers=voxtral,eleven,minimax,fish   (default: all with keys present)
 *   --tagged                                  (add an ElevenLabs v3 variant with
 *                                              LLM-inserted audio tags)
 *   --out=/path/to/dir                        (default: ~/Documents/tts-bakeoff)
 *
 * Required env per provider:
 *   voxtral: MISTRAL_API_KEY (already in .env)
 *   eleven:  ELEVENLABS_API_KEY  (optional ELEVEN_VOICE_ID, default Brian)
 *   minimax: MINIMAX_API_KEY + MINIMAX_GROUP_ID (optional MINIMAX_VOICE_ID)
 *   fish:    FISH_API_KEY (optional FISH_REFERENCE_ID; auto-picks a top English
 *            voice and prints alternatives if unset)
 * Plus GOOGLE_GENERATIVE_AI_API_KEY for retrieval rating + narration cleaning.
 *
 * Every output is loudness-matched (two-pass linear loudnorm to -16 LUFS) so
 * "louder" can't masquerade as "better" while listening.
 */
import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { Logger } from "@micthiesen/mitools/logging";
import { Mistral } from "@mistralai/mistralai";
import { generateText } from "ai";
import {
  Clock,
  Data,
  Duration,
  Effect,
  Result,
  Random,
  Ref,
  Schedule,
  Schema,
} from "effect";
import got from "got";
import { getPressPodsCleaningModel } from "../ai/registry.js";
import { getCleanedArticle } from "../press-pods/agents/cleaner.js";
import CostCounter from "../press-pods/costs.js";
import type { PressPodsError } from "../press-pods/effect.js";
import { buildFinalText } from "../press-pods/formatting/index.js";
import { getArticleFromUrl } from "../press-pods/retrievers/index.js";
import config from "../utils/config.js";
import { AppLayer } from "../effect/appRuntime.js";
import type { TaskServices } from "../task-runs/registry.js";

// Voxtral is retired from the production pipeline but kept here as the bake-off
// baseline, so these are inlined rather than imported from the prod speech dir.
const VOXTRAL_MODEL = "voxtral-mini-tts-2603";
const VOXTRAL_VOICE = {
  id: "c69964a6-ab8b-4f8a-9465-ec0925096ec8",
  name: "Paul - Neutral",
};

const logger = Logger.named("Bakeoff");

// ---------------------------------------------------------------------------
// Provider voice defaults (override via env)
// ---------------------------------------------------------------------------
const ELEVEN_VOICE_ID = process.env.ELEVEN_VOICE_ID ?? "nPczCjzI2devNBz1zQrb"; // Brian (premade narrator)
const ELEVEN_SEED = 4242;
const ELEVEN_CHUNK_TARGET = 900; // chars; v3 sweet spot is 500-800, cap 5k
const ELEVEN_CHUNK_MAX = 1500;
const MINIMAX_VOICE_ID = process.env.MINIMAX_VOICE_ID ?? "English_expressive_narrator";
const FISH_MODEL = process.env.FISH_MODEL ?? "s2.1-pro";
// "Alex - expressive narrator" — picked from /model list; override with FISH_REFERENCE_ID
const FISH_REFERENCE_ID =
  process.env.FISH_REFERENCE_ID ?? "f772ea09ebe04f66bd3e4a2be1e17329";

type ProviderName = "voxtral" | "eleven" | "eleven-tagged" | "minimax" | "fish";

interface SynthResult {
  provider: ProviderName;
  rawFile: string;
  seconds: number;
  notes: string[];
}

class BakeoffError extends Data.TaggedError("BakeoffError")<{
  operation: string;
  cause: unknown;
}> {}

class ProviderError extends Data.TaggedError("ProviderError")<{
  provider: ProviderName;
  operation: string;
  cause: unknown;
}> {}

class AudioProcessError extends Data.TaggedError("AudioProcessError")<{
  operation: string;
  cause: unknown;
}> {}

class MinimaxPending extends Data.TaggedError("MinimaxPending")<{}> {}

type ToolError = BakeoffError | ProviderError | AudioProcessError;

const fromPromise = <A>(
  operation: string,
  evaluate: (signal: AbortSignal) => PromiseLike<A>,
) =>
  Effect.tryPromise({
    try: evaluate,
    catch: (cause) => new BakeoffError({ operation, cause }),
  });

const providerPromise = <A>(
  provider: ProviderName,
  operation: string,
  evaluate: (signal: AbortSignal) => PromiseLike<A>,
) =>
  Effect.tryPromise({
    try: evaluate,
    catch: (cause) => new ProviderError({ provider, operation, cause }),
  });

const decodeExternal = <A, I>(
  operation: string,
  schema: Schema.Codec<A, I>,
  value: unknown,
): Effect.Effect<A, BakeoffError> =>
  Schema.decodeUnknownEffect(schema)(value).pipe(
    Effect.mapError((cause) => new BakeoffError({ operation, cause })),
  );

// ---------------------------------------------------------------------------
// ffmpeg helpers
// ---------------------------------------------------------------------------
function executeFile(
  executable: string,
  args: string[],
): Effect.Effect<{ stdout: string; stderr: string }, AudioProcessError> {
  return Effect.callback((resume) => {
    const child = execFile(
      executable,
      args,
      { maxBuffer: 64 * 1024 * 1024 },
      (error, stdout, stderr) => {
        if (error) {
          resume(
            Effect.fail(
              new AudioProcessError({
                operation: `${executable} ${args.join(" ")}`,
                cause: error,
              }),
            ),
          );
          return;
        }
        resume(Effect.succeed({ stdout, stderr }));
      },
    );
    return Effect.sync(() => child.kill("SIGTERM"));
  });
}

const ffmpeg = (args: string[]): Effect.Effect<string, AudioProcessError> =>
  executeFile("ffmpeg", ["-hide_banner", "-y", ...args]).pipe(
    Effect.map(({ stderr }) => stderr),
  );

const probeDuration = (file: string): Effect.Effect<number, AudioProcessError> =>
  executeFile("ffprobe", [
    "-v",
    "error",
    "-show_entries",
    "format=duration",
    "-of",
    "csv=p=0",
    file,
  ]).pipe(
    Effect.flatMap(({ stdout }) => {
      const duration = Number.parseFloat(stdout.trim());
      return Number.isFinite(duration)
        ? Effect.succeed(duration)
        : Effect.fail(
            new AudioProcessError({
              operation: `parse duration for ${file}`,
              cause: new Error(`Invalid ffprobe duration: ${stdout.trim()}`),
            }),
          );
    }),
  );

/**
 * Two-pass linear loudnorm. Emits an MP3 (final master, -16 LUFS by default) or
 * a 44.1k mono WAV when `wav` is set (per-chunk leveling before concat).
 */
function loudnessMatch(
  inFile: string,
  outFile: string,
  { target = -16, wav = false }: { target?: number; wav?: boolean } = {},
): Effect.Effect<void, AudioProcessError | BakeoffError> {
  const LoudnormSchema = Schema.Struct({
    input_i: Schema.String,
    input_tp: Schema.String,
    input_lra: Schema.String,
    input_thresh: Schema.String,
    target_offset: Schema.String,
  });
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
      return yield* Effect.fail(
        new AudioProcessError({
          operation: `measure loudness for ${inFile}`,
          cause: new Error("loudnorm pass 1 produced no JSON"),
        }),
      );
    }
    const parsed = yield* Effect.try({
      try: () => JSON.parse(jsonMatch[0]) as unknown,
      catch: (cause) => new BakeoffError({ operation: "parse loudnorm JSON", cause }),
    });
    const m = yield* decodeExternal("decode loudnorm response", LoudnormSchema, parsed);
    const filter =
      `loudnorm=${spec}:linear=true` +
      `:measured_I=${m.input_i}:measured_TP=${m.input_tp}` +
      `:measured_LRA=${m.input_lra}:measured_thresh=${m.input_thresh}` +
      `:offset=${m.target_offset},aresample=44100`;
    const encode = wav ? ["-c:a", "pcm_s16le"] : ["-c:a", "libmp3lame", "-b:a", "128k"];
    yield* ffmpeg([
      "-i",
      inFile,
      "-af",
      filter,
      "-ar",
      "44100",
      "-ac",
      "1",
      ...encode,
      outFile,
    ]);
  });
}

const CHUNK_EDGE_FADE = 0.012; // 12ms fade in/out at each chunk edge kills seam clicks

const removeIfPresent = (file: string): Effect.Effect<void> =>
  Effect.promise(() => fs.unlink(file)).pipe(Effect.ignore);

/**
 * Turn a raw chunk MP3 into a concat-ready WAV: trim edge silence, apply short
 * edge fades so butt-joins don't click, then per-chunk loudness-normalize to
 * -19 LUFS so no chunk sits quieter than its neighbors. The areverse sandwich
 * lets us trim + fade the trailing edge without knowing the duration.
 */
function prepareChunkWav(
  inFile: string,
  outFile: string,
): Effect.Effect<void, ToolError> {
  const trimmed = outFile.replace(/\.wav$/, ".trim.wav");
  const edge =
    "silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.15," +
    `afade=t=in:st=0:d=${CHUNK_EDGE_FADE},` +
    "areverse," +
    "silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.25," +
    `afade=t=in:st=0:d=${CHUNK_EDGE_FADE},` +
    "areverse";
  return Effect.acquireUseRelease(
    Effect.succeed(trimmed),
    () =>
      ffmpeg(["-i", inFile, "-af", edge, "-ar", "44100", "-ac", "1", trimmed]).pipe(
        Effect.andThen(loudnessMatch(trimmed, outFile, { target: -19, wav: true })),
      ),
    removeIfPresent,
  );
}

function makeSilenceWav(
  outFile: string,
  seconds: number,
): Effect.Effect<void, AudioProcessError> {
  return ffmpeg([
    "-f",
    "lavfi",
    "-i",
    "anullsrc=r=44100:cl=mono",
    "-t",
    String(seconds),
    outFile,
  ]).pipe(Effect.asVoid);
}

/** Concat WAVs (already same format) via the concat demuxer into one WAV. */
function concatWavs(files: string[], outFile: string): Effect.Effect<void, ToolError> {
  return Effect.gen(function* () {
    const now = yield* Clock.currentTimeMillis;
    const nonce = yield* Random.nextInt;
    const listPath = path.join(os.tmpdir(), `bakeoff_list_${now}_${nonce}.txt`);
    yield* Effect.acquireUseRelease(
      fromPromise("write concat list", () =>
        fs.writeFile(
          listPath,
          files.map((f) => `file '${f.replace(/'/g, "'\\''")}'`).join("\n"),
        ),
      ),
      () =>
        ffmpeg([
          "-f",
          "concat",
          "-safe",
          "0",
          "-i",
          listPath,
          "-c",
          "copy",
          outFile,
        ]).pipe(Effect.asVoid),
      () => removeIfPresent(listPath),
    );
  });
}

// ---------------------------------------------------------------------------
// Text chunking (paragraph-first, sentence fallback, never mid-sentence)
// ---------------------------------------------------------------------------
function splitSentences(paragraph: string): string[] {
  return paragraph.split(/(?<=[.!?…])\s+/).filter((s) => s.trim().length > 0);
}

function chunkText(text: string, target: number, max: number): string[] {
  const paragraphs = text
    .split(/\n\s*\n/)
    .map((p) => p.trim())
    .filter((p) => p.length > 0);
  const units: string[] = [];
  for (const p of paragraphs) {
    if (p.length <= max) units.push(p);
    else {
      // Oversized paragraph: emit sentence groups up to target
      let buf = "";
      for (const s of splitSentences(p)) {
        if (buf && buf.length + s.length + 1 > target) {
          units.push(buf);
          buf = s;
        } else buf = buf ? `${buf} ${s}` : s;
      }
      if (buf) units.push(buf);
    }
  }
  // Greedily merge whole units up to target
  const chunks: string[] = [];
  let buf = "";
  for (const u of units) {
    if (buf && buf.length + u.length + 2 > target) {
      chunks.push(buf);
      buf = u;
    } else buf = buf ? `${buf}\n\n${u}` : u;
  }
  if (buf) chunks.push(buf);
  return chunks;
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------
function synthVoxtral(
  text: string,
  outDir: string,
): Effect.Effect<SynthResult, ToolError, Logger> {
  const ResponseSchema = Schema.Struct({ audioData: Schema.String });
  return Effect.gen(function* () {
    const start = yield* Clock.currentTimeMillis;
    const client = new Mistral({ apiKey: config.MISTRAL_API_KEY });
    const response = yield* providerPromise("voxtral", "synthesize", () =>
      client.audio.speech.complete(
        {
          model: VOXTRAL_MODEL,
          input: text,
          voiceId: VOXTRAL_VOICE.id,
          responseFormat: "mp3",
          stream: false,
        },
        { timeoutMs: 15 * 60 * 1000 },
      ),
    );
    const decoded = yield* decodeExternal(
      "decode voxtral response",
      ResponseSchema,
      response,
    );
    const rawFile = path.join(outDir, "voxtral-raw.mp3");
    yield* providerPromise("voxtral", "write audio", () =>
      fs.writeFile(rawFile, Buffer.from(decoded.audioData, "base64")),
    );
    const end = yield* Clock.currentTimeMillis;
    return {
      provider: "voxtral",
      rawFile,
      seconds: (end - start) / 1000,
      notes: [`voice=${VOXTRAL_VOICE.name}`, `model=${VOXTRAL_MODEL}`],
    };
  });
}

function synthEleven(
  text: string,
  outDir: string,
  variant: "eleven" | "eleven-tagged",
): Effect.Effect<SynthResult, ToolError, Logger> {
  return Effect.gen(function* () {
    const apiKey = process.env.ELEVENLABS_API_KEY;
    if (!apiKey) {
      return yield* Effect.fail(
        new ProviderError({
          provider: variant,
          operation: "configure",
          cause: new Error("ELEVENLABS_API_KEY not set"),
        }),
      );
    }
    const start = yield* Clock.currentTimeMillis;
    const chunks = chunkText(text, ELEVEN_CHUNK_TARGET, ELEVEN_CHUNK_MAX);
    yield* logger.info(`[${variant}] synthesizing ${chunks.length} chunks`);
    const random = yield* Random.nextInt;
    const nonce = `${start}_${random}`;
    const gapWav = path.join(os.tmpdir(), `bakeoff_gap_${nonce}.wav`);
    const joined = path.join(os.tmpdir(), `bakeoff_el_join_${nonce}.wav`);
    const chunkFiles: string[] = [];

    const rawFile = yield* makeSilenceWav(gapWav, 0.75).pipe(
      Effect.andThen(
        Effect.gen(function* () {
          const wavs = yield* Effect.forEach(
            chunks,
            (chunk, index) =>
              Effect.gen(function* () {
                const mp3 = path.join(os.tmpdir(), `bakeoff_el_${nonce}_${index}.mp3`);
                const wav = mp3.replace(/\.mp3$/, ".wav");
                chunkFiles.push(mp3, wav);
                const audio = yield* providerPromise(
                  variant,
                  `synthesize chunk ${index + 1}`,
                  (signal) =>
                    got
                      .post(
                        `https://api.elevenlabs.io/v1/text-to-speech/${ELEVEN_VOICE_ID}?output_format=mp3_44100_128`,
                        {
                          headers: { "xi-api-key": apiKey },
                          json: {
                            text: chunk,
                            model_id: "eleven_v3",
                            seed: ELEVEN_SEED,
                            voice_settings: {
                              stability: 0.5,
                              use_speaker_boost: true,
                            },
                          },
                          timeout: { request: 5 * 60 * 1000 },
                          signal,
                        },
                      )
                      .buffer(),
                );
                yield* providerPromise(variant, `write chunk ${index + 1}`, () =>
                  fs.writeFile(mp3, audio),
                );
                yield* prepareChunkWav(mp3, wav);
                yield* removeIfPresent(mp3);
                yield* logger.info(
                  `[${variant}] chunk ${index + 1}/${chunks.length} done`,
                );
                return wav;
              }),
            { concurrency: 2 },
          );
          const withGaps = wavs.flatMap((wav, index) =>
            index === 0 ? [wav] : [gapWav, wav],
          );
          yield* concatWavs(withGaps, joined);
          const output = path.join(outDir, `${variant}-raw.mp3`);
          yield* ffmpeg(["-i", joined, "-c:a", "libmp3lame", "-b:a", "128k", output]);
          return output;
        }),
      ),
      Effect.ensuring(
        Effect.suspend(() =>
          Effect.forEach([...chunkFiles, gapWav, joined], removeIfPresent, {
            discard: true,
          }),
        ),
      ),
    );
    const end = yield* Clock.currentTimeMillis;
    return {
      provider: variant,
      rawFile,
      seconds: (end - start) / 1000,
      notes: [
        `voice=${ELEVEN_VOICE_ID}`,
        `model=eleven_v3 (Natural, seed=${ELEVEN_SEED})`,
        `chunks=${chunks.length} @ ~${ELEVEN_CHUNK_TARGET} chars, per-chunk -19 LUFS, 12ms edge fades, 0.75s gaps`,
      ],
    };
  });
}

function synthMinimaxEffect(
  text: string,
  outDir: string,
): Effect.Effect<SynthResult, ToolError, Logger> {
  const IdSchema = Schema.Union([Schema.String, Schema.Number]);
  const CreateSchema = Schema.Struct({
    base_resp: Schema.optional(
      Schema.Struct({
        status_code: Schema.optional(Schema.Number),
        status_msg: Schema.optional(Schema.String),
      }),
    ),
    task_id: Schema.optional(IdSchema),
    data: Schema.optional(Schema.Struct({ task_id: Schema.optional(IdSchema) })),
  });
  const PollSchema = Schema.Struct({
    status: Schema.optional(Schema.String),
    file_id: Schema.optional(IdSchema),
    data: Schema.optional(
      Schema.Struct({
        status: Schema.optional(Schema.String),
        file_id: Schema.optional(IdSchema),
      }),
    ),
  });
  const DownloadSchema = Schema.Struct({
    download_url: Schema.optional(Schema.String),
    file: Schema.optional(
      Schema.Struct({ download_url: Schema.optional(Schema.String) }),
    ),
    data: Schema.optional(
      Schema.Struct({ download_url: Schema.optional(Schema.String) }),
    ),
  });
  return Effect.gen(function* () {
    const apiKey = process.env.MINIMAX_API_KEY;
    const groupId = process.env.MINIMAX_GROUP_ID;
    if (!apiKey || !groupId) {
      return yield* Effect.fail(
        new ProviderError({
          provider: "minimax",
          operation: "configure",
          cause: new Error("MINIMAX_API_KEY / MINIMAX_GROUP_ID not set"),
        }),
      );
    }
    const start = yield* Clock.currentTimeMillis;
    const headers = { Authorization: `Bearer ${apiKey}` };
    const createUnknown = yield* providerPromise("minimax", "create task", (signal) =>
      got
        .post(`https://api.minimax.io/v1/t2a_async_v2?GroupId=${groupId}`, {
          headers,
          json: {
            model: "speech-2.8-hd",
            text,
            language_boost: "auto",
            voice_setting: { voice_id: MINIMAX_VOICE_ID, speed: 1 },
            audio_setting: {
              audio_sample_rate: 44100,
              bitrate: 128000,
              format: "mp3",
              channel: 1,
            },
          },
          timeout: { request: 60_000 },
          signal,
        })
        .json<unknown>(),
    );
    const createRes = yield* decodeExternal(
      "decode minimax create response",
      CreateSchema,
      createUnknown,
    );
    yield* logger.info("[minimax] task created", { createRes });
    const baseResp = createRes.base_resp;
    if (baseResp && baseResp.status_code !== 0) {
      return yield* Effect.fail(
        new ProviderError({
          provider: "minimax",
          operation: "create minimax task",
          cause: new Error(
            `minimax create failed (${baseResp.status_code}): ${baseResp.status_msg}`,
          ),
        }),
      );
    }
    const taskId = createRes.task_id ?? createRes.data?.task_id;
    if (taskId === undefined || taskId === 0) {
      return yield* Effect.fail(
        new ProviderError({
          provider: "minimax",
          operation: "create minimax task",
          cause: new Error(
            `minimax: no task_id in response: ${JSON.stringify(createRes)}`,
          ),
        }),
      );
    }

    const attempt = yield* Ref.make(0);
    const poll = providerPromise("minimax", "poll task", (signal) =>
      got
        .get(
          `https://api.minimax.io/v1/query/t2a_async_query_v2?GroupId=${groupId}&task_id=${taskId}`,
          { headers, timeout: { request: 60_000 }, signal },
        )
        .json<unknown>(),
    ).pipe(
      Effect.flatMap((response) =>
        decodeExternal("decode minimax poll response", PollSchema, response),
      ),
      Effect.flatMap((response) => {
        const status = response.status ?? response.data?.status ?? "";
        return Ref.getAndUpdate(attempt, (count) => count + 1).pipe(
          Effect.tap((count) =>
            count % 6 === 0
              ? logger.info(`[minimax] poll status=${status || "?"}`, { response })
              : Effect.void,
          ),
          Effect.flatMap(
            (): Effect.Effect<string | number, MinimaxPending | ProviderError> => {
              if (/fail|expired/i.test(status)) {
                return Effect.fail(
                  new ProviderError({
                    provider: "minimax",
                    operation: "poll task",
                    cause: new Error(`Task failed with status ${status}`),
                  }),
                );
              }
              const fileId = /success/i.test(status)
                ? (response.file_id ?? response.data?.file_id)
                : undefined;
              return fileId === undefined
                ? Effect.fail(new MinimaxPending())
                : Effect.succeed(fileId);
            },
          ),
        );
      }),
    );
    const fileId = yield* poll.pipe(
      Effect.retry({
        schedule: Schedule.addDelay(Schedule.recurs(179), () =>
          Effect.succeed(Duration.seconds(10)),
        ),
        while: (error) => error._tag === "MinimaxPending",
      }),
      Effect.catchTag("MinimaxPending", () =>
        new ProviderError({
          provider: "minimax",
          operation: "poll task",
          cause: new Error("minimax: timed out waiting for task"),
        }).pipe(Effect.fail),
      ),
    );
    const fileRes = yield* providerPromise("minimax", "download audio", (signal) =>
      got.get(
        `https://api.minimax.io/v1/files/retrieve_content?GroupId=${groupId}&file_id=${fileId}`,
        { headers, timeout: { request: 5 * 60 * 1000 }, signal },
      ),
    );
    let audio: Uint8Array = fileRes.rawBody;
    if (fileRes.headers["content-type"]?.includes("application/json")) {
      const parsed = yield* Effect.try({
        try: () => JSON.parse(fileRes.rawBody.toString()) as unknown,
        catch: (cause) =>
          new ProviderError({
            provider: "minimax",
            operation: "parse download response",
            cause,
          }),
      });
      const meta = yield* decodeExternal(
        "decode minimax download response",
        DownloadSchema,
        parsed,
      );
      const url =
        meta.file?.download_url ?? meta.download_url ?? meta.data?.download_url;
      if (!url) {
        return yield* Effect.fail(
          new ProviderError({
            provider: "minimax",
            operation: "decode download response",
            cause: new Error("minimax: no download_url"),
          }),
        );
      }
      audio = yield* providerPromise("minimax", "download audio URL", (signal) =>
        got.get(url, { timeout: { request: 5 * 60 * 1000 }, signal }).buffer(),
      );
    }
    const rawFile = path.join(outDir, "minimax-raw.mp3");
    yield* providerPromise("minimax", "write audio", () =>
      fs.writeFile(rawFile, audio),
    );
    const end = yield* Clock.currentTimeMillis;
    return {
      provider: "minimax",
      rawFile,
      seconds: (end - start) / 1000,
      notes: [
        `voice=${MINIMAX_VOICE_ID}`,
        "model=speech-2.8-hd (async, single request)",
      ],
    };
  });
}

function synthFish(
  text: string,
  outDir: string,
): Effect.Effect<SynthResult, ProviderError> {
  return Effect.gen(function* () {
    const apiKey = process.env.FISH_API_KEY;
    if (!apiKey) {
      return yield* Effect.fail(
        new ProviderError({
          provider: "fish",
          operation: "configure",
          cause: new Error("FISH_API_KEY not set"),
        }),
      );
    }
    const start = yield* Clock.currentTimeMillis;
    const referenceId = FISH_REFERENCE_ID;
    const audio = yield* providerPromise("fish", "synthesize", (signal) =>
      got
        .post("https://api.fish.audio/v1/tts", {
          headers: { Authorization: `Bearer ${apiKey}`, model: FISH_MODEL },
          json: {
            text,
            reference_id: referenceId,
            format: "mp3",
            normalize: true,
            latency: "normal",
            temperature: 0.7,
          },
          timeout: { request: 15 * 60 * 1000 },
          signal,
        })
        .buffer(),
    );
    const rawFile = path.join(outDir, "fish-raw.mp3");
    yield* providerPromise("fish", "write audio", () => fs.writeFile(rawFile, audio));
    const end = yield* Clock.currentTimeMillis;
    return {
      provider: "fish",
      rawFile,
      seconds: (end - start) / 1000,
      notes: [
        `voice=${referenceId}`,
        `model=${FISH_MODEL} (single request, vendor chunking)`,
      ],
    };
  });
}

// ---------------------------------------------------------------------------
// Optional: LLM "director" pass adding ElevenLabs v3 audio tags
// ---------------------------------------------------------------------------
function addAudioTags(text: string): Effect.Effect<string, ProviderError> {
  return Effect.gen(function* () {
    const { model } = getPressPodsCleaningModel();
    const response = yield* providerPromise(
      "eleven-tagged",
      "add audio tags",
      (signal) =>
        generateText({
          model,
          abortSignal: signal,
          system: `You annotate a narration script with ElevenLabs v3 audio tags to make a single podcast-host voice more engaging. Insert inline square-bracket tags SPARINGLY.

Rules:
- Density ceiling: at most one tag per 3-4 sentences. Many paragraphs should have none. Never more than one tag per sentence.
- Place each tag immediately before the words it should affect, mid-paragraph is fine.
- Allowed tags only: [thoughtful] [curious] [excited] [warmly] [serious] [sarcastic] [amused] [impressed] [reassuring] [deadpan] [sighs] [chuckles] [exhales] [short pause] [pause] [drawn out]
- BANNED: sound effects, accents, [laughs] at nothing, [whispers], [dramatic pause], anything not in the allowed list.
- Preserve every spoken word exactly. Do not add, remove, or reorder any words. Only insert tags.
- Match tags to the actual content: [curious] for questions/setups, [serious] for grave facts, [amused]/[chuckles] only where the text itself is genuinely wry.
- Return ONLY the annotated script, no commentary.`,
          prompt: text,
        }),
    );
    const out = yield* Schema.decodeUnknownEffect(Schema.String)(response.text).pipe(
      Effect.mapError(
        (cause) =>
          new ProviderError({
            provider: "eleven-tagged",
            operation: "decode tagged narration",
            cause,
          }),
      ),
    );
    return out.trim();
  });
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------
function slugify(s: string): string {
  return s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 60);
}

function detectProviders(): ProviderName[] {
  const p: ProviderName[] = [];
  if (config.MISTRAL_API_KEY) p.push("voxtral");
  if (process.env.ELEVENLABS_API_KEY) p.push("eleven");
  if (process.env.MINIMAX_API_KEY && process.env.MINIMAX_GROUP_ID) p.push("minimax");
  if (process.env.FISH_API_KEY) p.push("fish");
  return p;
}

function runProvidersEffect({
  content,
  outDir,
  title,
  url,
  providers,
  tagged,
}: {
  content: string;
  outDir: string;
  title: string;
  url?: string;
  providers: ProviderName[];
  tagged: boolean;
}): Effect.Effect<void, ToolError, Logger> {
  return Effect.gen(function* () {
    yield* fromPromise("create output directory", () =>
      fs.mkdir(outDir, { recursive: true }),
    );
    yield* fromPromise("write narration", () =>
      fs.writeFile(path.join(outDir, "narration.md"), content),
    );

    let taggedContent: string | undefined;
    if (tagged && providers.includes("eleven")) {
      taggedContent = yield* addAudioTags(content);
      yield* fromPromise("write tagged narration", () =>
        fs.writeFile(path.join(outDir, "narration-tagged.md"), taggedContent!),
      );
      yield* logger.info("Tagged narration variant ready");
    }

    const jobs: Array<Effect.Effect<SynthResult, ToolError, Logger>> = [];
    if (providers.includes("voxtral")) jobs.push(synthVoxtral(content, outDir));
    if (providers.includes("eleven")) jobs.push(synthEleven(content, outDir, "eleven"));
    if (taggedContent) {
      const t = taggedContent;
      jobs.push(synthEleven(t, outDir, "eleven-tagged"));
    }
    if (providers.includes("minimax")) jobs.push(synthMinimaxEffect(content, outDir));
    if (providers.includes("fish")) jobs.push(synthFish(content, outDir));

    const settled = yield* Effect.forEach(jobs, (job) => job.pipe(Effect.result), {
      concurrency: "unbounded",
    });
    const results: SynthResult[] = [];
    for (const s of settled) {
      if (Result.isSuccess(s)) results.push(s.success);
      else yield* logger.error(`Provider failed: ${s.failure.cause}`);
    }

    // Loudness-match everything and summarize
    const lines: string[] = [
      `# ${title}`,
      "",
      ...(url ? [`- URL: ${url}`] : []),
      `- Narration: ${content.length} chars`,
      "",
      "| provider | duration | synth time | notes |",
      "|---|---|---|---|",
    ];
    for (const r of results) {
      const outFile = path.join(outDir, `${r.provider}.mp3`);
      const mastered = yield* loudnessMatch(r.rawFile, outFile).pipe(
        Effect.andThen(probeDuration(outFile)),
        Effect.result,
      );
      if (Result.isSuccess(mastered)) {
        lines.push(
          `| ${r.provider} | ${(mastered.success / 60).toFixed(1)} min | ${r.seconds.toFixed(0)}s | ${r.notes.join("; ")} |`,
        );
      } else {
        yield* logger.error(
          `Loudness match failed for ${r.provider}: ${mastered.failure.cause}`,
        );
        lines.push(
          `| ${r.provider} | ? | ${r.seconds.toFixed(0)}s | loudnorm FAILED, use ${path.basename(r.rawFile)} |`,
        );
      }
    }
    const summary = lines.join("\n");
    const summaryPath = path.join(outDir, "summary.md");
    const existing = yield* fromPromise("read prior summary", () =>
      fs.readFile(summaryPath, "utf8"),
    ).pipe(Effect.option);
    yield* fromPromise("write summary", () =>
      fs.writeFile(
        summaryPath,
        existing._tag === "Some"
          ? `${existing.value}\n\n## Rerun\n\n${summary}`
          : summary,
      ),
    );
    yield* logger.info(`\n${summary}\n\nOutput: ${outDir}`);
  });
}

function mainEffect(): Effect.Effect<void, ToolError | PressPodsError, TaskServices> {
  return Effect.gen(function* () {
    const args = process.argv.slice(2);
    const urls = args.filter((a) => !a.startsWith("--"));
    const providersFlag = args.find((a) => a.startsWith("--providers="));
    const outFlag = args.find((a) => a.startsWith("--out="));
    const narrationFlag = args.find((a) => a.startsWith("--narration="));
    const tagged = args.includes("--tagged");
    if (urls.length === 0 && !narrationFlag) {
      return yield* Effect.fail(
        new BakeoffError({
          operation: "parse arguments",
          cause: new Error(
            "Usage: bun src/tools/tts-bakeoff.ts <article-url> [...urls] [--providers=...] [--tagged] [--out=dir]\n" +
              "   or: bun src/tools/tts-bakeoff.ts --narration=path/to/narration.md [--providers=...]",
          ),
        }),
      );
    }

    const ProviderSchema = Schema.Literals([
      "voxtral",
      "eleven",
      "eleven-tagged",
      "minimax",
      "fish",
    ]);
    const providers: ProviderName[] = providersFlag
      ? [
          ...(yield* decodeExternal(
            "decode providers argument",
            Schema.Array(ProviderSchema),
            providersFlag.split("=")[1].split(","),
          )),
        ]
      : detectProviders();
    const outRoot =
      outFlag?.split("=")[1] ?? path.join(os.homedir(), "Documents", "tts-bakeoff");
    yield* logger.info(
      `Providers: ${providers.join(", ")}${tagged ? " + eleven-tagged" : ""}`,
    );

    // Re-run providers against an existing narration file (identical text, so
    // late entrants stay comparable with earlier outputs in the same directory).
    if (narrationFlag) {
      const narrationPath = narrationFlag.split("=")[1];
      const content = yield* fromPromise("read narration", () =>
        fs.readFile(narrationPath, "utf8"),
      );
      const outDir = path.dirname(path.resolve(narrationPath));
      const title = path.basename(outDir);
      yield* runProvidersEffect({ content, outDir, title, providers, tagged });
      return;
    }

    yield* Effect.forEach(
      urls,
      (url) =>
        Effect.gen(function* () {
          yield* logger.info(`=== Article: ${url}`);
          const costCounter = new CostCounter();
          const { article, metadata } = yield* getArticleFromUrl(
            url,
            costCounter,
            logger,
          );
          const title = metadata.info.title ?? article.title ?? url;
          const text = buildFinalText({
            title,
            domain: metadata.info.publication ?? article.domain,
            author: metadata.info.author ?? article.author ?? "Anonymous",
            coauthors: metadata.info.coauthors,
            datePublished: metadata.info.publishedAtISO ?? article.publishedAt,
            text: article.text,
          });
          const { content } = yield* getCleanedArticle(
            { ...article, text },
            costCounter,
          );
          yield* logger.info(`Narration ready: ${content.length} chars`);

          yield* runProvidersEffect({
            content,
            outDir: path.join(outRoot, slugify(title)),
            title,
            url,
            providers: [...providers],
            tagged,
          });
        }),
      { concurrency: 1, discard: true },
    );
  });
}

await Effect.runPromise(mainEffect().pipe(Effect.provide(AppLayer)));
