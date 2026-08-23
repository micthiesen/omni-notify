import { spawn } from "node:child_process";

const USER_AGENT = "OpenAI File Downloader, XaiImageApiFetch/1.0";
const SAMPLE_RATE = 16_000;
const MAX_STDERR_BYTES = 16_384;

export interface CapturedAudio {
  samples: Float32Array;
  sampleRate: number;
  durationSeconds: number;
}

type ResolvedMedia = {
  url: string;
  http_headers?: Record<string, string>;
};

function run(
  command: string,
  args: string[],
  options: { timeoutMs: number; maxStdoutBytes: number },
): Promise<{ stdout: Buffer; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: ["ignore", "pipe", "pipe"] });
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let settled = false;
    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (error) reject(error);
      else
        resolve({
          stdout: Buffer.concat(stdout),
          stderr: Buffer.concat(stderr).toString(),
        });
    };
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      finish(new Error(`${command} timed out after ${options.timeoutMs}ms`));
    }, options.timeoutMs);
    child.on("error", (error) => finish(error));
    child.stdout.on("data", (chunk: Buffer) => {
      stdoutBytes += chunk.length;
      if (stdoutBytes > options.maxStdoutBytes) {
        child.kill("SIGKILL");
        finish(new Error(`${command} exceeded output limit`));
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
          new Error(
            `${command} exited ${code ?? signal}: ${Buffer.concat(stderr).toString().slice(-500)}`,
          ),
        );
    });
  });
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

  private async resolve(streamUrl: string): Promise<ResolvedMedia> {
    const cached = this.cache.get(streamUrl);
    if (cached && cached.expiresAt > Date.now()) return cached.media;
    const { stdout } = await run(
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
    const parsed = JSON.parse(stdout.toString()) as ResolvedMedia;
    if (!parsed.url || typeof parsed.url !== "string") {
      throw new Error("yt-dlp returned no playable media URL");
    }
    this.cache.set(streamUrl, { media: parsed, expiresAt: Date.now() + 10 * 60_000 });
    return parsed;
  }

  public async capture(
    streamUrl: string,
    durationSeconds: number,
    seekSeconds?: number,
  ): Promise<CapturedAudio> {
    let media: ResolvedMedia;
    try {
      media = await this.resolve(streamUrl);
      return await this.captureResolved(media, durationSeconds, seekSeconds);
    } catch (error) {
      this.cache.delete(streamUrl);
      if (error instanceof SyntaxError) throw error;
      media = await this.resolve(streamUrl);
      return this.captureResolved(media, durationSeconds, seekSeconds);
    }
  }

  private async captureResolved(
    media: ResolvedMedia,
    durationSeconds: number,
    seekSeconds?: number,
  ): Promise<CapturedAudio> {
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
    const { stdout } = await run(this.ffmpegPath, args, {
      timeoutMs: Math.max(30_000, durationSeconds * 2_000 + 15_000),
      maxStdoutBytes: maxBytes,
    });
    const aligned = stdout.length - (stdout.length % 4);
    const copy = Uint8Array.from(stdout.subarray(0, aligned));
    const samples = new Float32Array(copy.buffer);
    if (samples.length < SAMPLE_RATE)
      throw new Error("Captured less than one second of audio");
    return {
      samples,
      sampleRate: SAMPLE_RATE,
      durationSeconds: samples.length / SAMPLE_RATE,
    };
  }
}
