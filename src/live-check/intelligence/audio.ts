import type { Effect as EffectType } from "effect/Effect";
import { spawn } from "node:child_process";
import { Clock, Data, Effect, Schema } from "effect";

const USER_AGENT = "OpenAI File Downloader, XaiImageApiFetch/1.0";
const SAMPLE_RATE = 16_000;
const MAX_STDERR_BYTES = 16_384;

export interface CapturedAudio {
  samples: Float32Array;
  sampleRate: number;
  durationSeconds: number;
}

const resolvedMediaSchema = Schema.Struct({
  url: Schema.String,
  http_headers: Schema.optional(Schema.Record(Schema.String, Schema.String)),
});
type ResolvedMedia = Schema.Schema.Type<typeof resolvedMediaSchema>;

export class AudioProcessError extends Data.TaggedError("AudioProcessError")<{
  readonly message: string;
  readonly retryable: boolean;
}> {}

export function runAudioProcess(
  command: string,
  args: string[],
  options: { timeoutMs: number; maxStdoutBytes: number },
): EffectType<{ stdout: Buffer; stderr: string }, AudioProcessError> {
  const process = Effect.callback<
    { stdout: Buffer; stderr: string },
    AudioProcessError
  >((resume) => {
    const child = spawn(command, args, { stdio: ["ignore", "pipe", "pipe"] });
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let settled = false;
    const finish = (error?: AudioProcessError) => {
      if (settled) return;
      settled = true;
      if (error) resume(Effect.fail(error));
      else
        resume(
          Effect.succeed({
            stdout: Buffer.concat(stdout),
            stderr: Buffer.concat(stderr).toString(),
          }),
        );
    };
    child.on("error", (cause) =>
      finish(new AudioProcessError({ message: cause.message, retryable: true })),
    );
    child.stdout.on("data", (chunk: Buffer) => {
      stdoutBytes += chunk.length;
      if (stdoutBytes > options.maxStdoutBytes) {
        child.kill("SIGKILL");
        finish(
          new AudioProcessError({
            message: `${command} exceeded output limit`,
            retryable: false,
          }),
        );
        return;
      }
      stdout.push(chunk);
    });
    child.stderr.on("data", (chunk: Buffer) => {
      if (stderrBytes >= MAX_STDERR_BYTES) return;
      const remaining = MAX_STDERR_BYTES - stderrBytes;
      stderr.push(chunk.subarray(0, remaining));
      stderrBytes += Math.min(chunk.length, remaining);
    });
    child.on("close", (code, signal) => {
      if (code === 0) finish();
      else
        finish(
          new AudioProcessError({
            message: `${command} exited ${code ?? signal}: ${Buffer.concat(stderr).toString().slice(-500)}`,
            retryable: true,
          }),
        );
    });
    return Effect.sync(() => {
      settled = true;
      child.kill("SIGKILL");
    });
  });
  return process.pipe(
    Effect.timeoutOrElse({
      duration: `${options.timeoutMs} millis`,
      orElse: () =>
        Effect.fail(
          new AudioProcessError({
            message: `${command} timed out after ${options.timeoutMs}ms`,
            retryable: false,
          }),
        ),
    }),
  );
}

function headerArgument(headers: Record<string, string> | undefined): string | null {
  if (!headers) return null;
  const lines = Object.entries(headers)
    .filter(([name, value]) => name.length > 0 && value.length > 0)
    .map(([name, value]) => `${name}: ${value.replace(/[\r\n]/g, " ")}`);
  return lines.length > 0 ? `${lines.join("\r\n")}\r\n` : null;
}

export class LivestreamAudioCapture {
  private readonly cache = new Map<
    string,
    { media: ResolvedMedia; expiresAt: number }
  >();

  public constructor(
    private readonly ytDlpPath = process.env.YT_DLP_PATH || "yt-dlp",
    private readonly ffmpegPath = process.env.FFMPEG_PATH || "ffmpeg",
  ) {}

  private resolve(
    streamUrl: string,
  ): EffectType<ResolvedMedia, AudioProcessError | AudioDecodeError> {
    return Effect.gen({ self: this }, function* () {
      const cached = this.cache.get(streamUrl);
      const now = yield* Clock.currentTimeMillis;
      if (cached && cached.expiresAt > now) return cached.media;
      const { stdout } = yield* runAudioProcess(
        this.ytDlpPath,
        [
          "--quiet",
          "--no-warnings",
          "--no-playlist",
          "--js-runtimes",
          "node",
          "--user-agent",
          USER_AGENT,
          "--format",
          "worstaudio[language^=en]/worstaudio/best",
          "--dump-single-json",
          streamUrl,
        ],
        { timeoutMs: 30_000, maxStdoutBytes: 4 * 1024 * 1024 },
      );
      const raw = yield* Effect.try({
        try: () => JSON.parse(stdout.toString()) as unknown,
        catch: (cause) =>
          new AudioDecodeError({ message: "yt-dlp returned invalid JSON", cause }),
      });
      const parsed = yield* Schema.decodeUnknownEffect(resolvedMediaSchema)(raw).pipe(
        Effect.mapError(
          (cause) =>
            new AudioDecodeError({
              message: "yt-dlp returned no playable media URL",
              cause,
            }),
        ),
      );
      this.cache.set(streamUrl, { media: parsed, expiresAt: now + 10 * 60_000 });
      return parsed;
    });
  }

  public captureEffect(
    streamUrl: string,
    durationSeconds: number,
    seekSeconds?: number,
  ): EffectType<CapturedAudio, AudioProcessError | AudioDecodeError> {
    const attempt = this.resolve(streamUrl).pipe(
      Effect.flatMap((media) =>
        this.captureResolved(media, durationSeconds, seekSeconds),
      ),
    );
    return attempt.pipe(
      Effect.catch((error) => {
        this.cache.delete(streamUrl);
        if (error instanceof AudioDecodeError || !error.retryable)
          return Effect.fail(error);
        return this.resolve(streamUrl).pipe(
          Effect.flatMap((media) =>
            this.captureResolved(media, durationSeconds, seekSeconds),
          ),
        );
      }),
    );
  }

  private captureResolved(
    media: ResolvedMedia,
    durationSeconds: number,
    seekSeconds?: number,
  ): EffectType<CapturedAudio, AudioProcessError> {
    const args = ["-hide_banner", "-loglevel", "error"];
    if (seekSeconds !== undefined && seekSeconds > 0) {
      args.push("-ss", String(seekSeconds));
    }
    const headers = headerArgument(media.http_headers);
    if (headers) args.push("-headers", headers);
    args.push(
      "-i",
      media.url,
      "-t",
      String(durationSeconds),
      "-vn",
      "-ac",
      "1",
      "-ar",
      String(SAMPLE_RATE),
      "-f",
      "f32le",
      "pipe:1",
    );
    const maxBytes = Math.ceil(durationSeconds * SAMPLE_RATE * 4 * 1.1);
    return runAudioProcess(this.ffmpegPath, args, {
      timeoutMs: Math.max(30_000, durationSeconds * 2_000 + 15_000),
      maxStdoutBytes: maxBytes,
    }).pipe(
      Effect.flatMap(({ stdout }) => {
        const aligned = stdout.length - (stdout.length % 4);
        const copy = Uint8Array.from(stdout.subarray(0, aligned));
        const samples = new Float32Array(copy.buffer);
        if (samples.length < SAMPLE_RATE) {
          return Effect.fail(
            new AudioProcessError({
              message: "Captured less than one second of audio",
              retryable: true,
            }),
          );
        }
        return Effect.succeed({
          samples,
          sampleRate: SAMPLE_RATE,
          durationSeconds: samples.length / SAMPLE_RATE,
        });
      }),
    );
  }
}

export class AudioDecodeError extends Data.TaggedError("AudioDecodeError")<{
  readonly message: string;
  readonly cause: unknown;
}> {}
