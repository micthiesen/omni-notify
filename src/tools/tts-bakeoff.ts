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
import { promisify } from "node:util";
import { Logger } from "@micthiesen/mitools/logging";
import { Mistral } from "@mistralai/mistralai";
import { generateText } from "ai";
import got from "got";
import { getPressPodsCleaningModel } from "../ai/registry.js";
import { getCleanedArticle } from "../press-pods/agents/cleaner.js";
import CostCounter from "../press-pods/costs.js";
import { buildFinalText } from "../press-pods/formatting/index.js";
import { getArticleFromUrl } from "../press-pods/retrievers/index.js";
import config from "../utils/config.js";

// Voxtral is retired from the production pipeline but kept here as the bake-off
// baseline, so these are inlined rather than imported from the prod speech dir.
const VOXTRAL_MODEL = "voxtral-mini-tts-2603";
const VOXTRAL_VOICE = {
  id: "c69964a6-ab8b-4f8a-9465-ec0925096ec8",
  name: "Paul - Neutral",
};

const execFileAsync = promisify(execFile);
const logger = new Logger("Bakeoff");

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

// ---------------------------------------------------------------------------
// ffmpeg helpers
// ---------------------------------------------------------------------------
async function ffmpeg(args: string[]): Promise<string> {
  const { stderr } = await execFileAsync("ffmpeg", ["-hide_banner", "-y", ...args], {
    maxBuffer: 64 * 1024 * 1024,
  });
  return stderr;
}

async function probeDuration(file: string): Promise<number> {
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
 * Two-pass linear loudnorm. Emits an MP3 (final master, -16 LUFS by default) or
 * a 44.1k mono WAV when `wav` is set (per-chunk leveling before concat).
 */
async function loudnessMatch(
  inFile: string,
  outFile: string,
  { target = -16, wav = false }: { target?: number; wav?: boolean } = {},
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
  if (!jsonMatch) throw new Error(`loudnorm pass 1 produced no JSON for ${inFile}`);
  const m = JSON.parse(jsonMatch[0]);
  const filter =
    `loudnorm=${spec}:linear=true` +
    `:measured_I=${m.input_i}:measured_TP=${m.input_tp}` +
    `:measured_LRA=${m.input_lra}:measured_thresh=${m.input_thresh}` +
    `:offset=${m.target_offset},aresample=44100`;
  const encode = wav ? ["-c:a", "pcm_s16le"] : ["-c:a", "libmp3lame", "-b:a", "128k"];
  await ffmpeg([
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
}

const CHUNK_EDGE_FADE = 0.012; // 12ms fade in/out at each chunk edge kills seam clicks

/**
 * Turn a raw chunk MP3 into a concat-ready WAV: trim edge silence, apply short
 * edge fades so butt-joins don't click, then per-chunk loudness-normalize to
 * -19 LUFS so no chunk sits quieter than its neighbors. The areverse sandwich
 * lets us trim + fade the trailing edge without knowing the duration.
 */
async function prepareChunkWav(inFile: string, outFile: string): Promise<void> {
  const trimmed = outFile.replace(/\.wav$/, ".trim.wav");
  const edge =
    "silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.15," +
    `afade=t=in:st=0:d=${CHUNK_EDGE_FADE},` +
    "areverse," +
    "silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.25," +
    `afade=t=in:st=0:d=${CHUNK_EDGE_FADE},` +
    "areverse";
  await ffmpeg(["-i", inFile, "-af", edge, "-ar", "44100", "-ac", "1", trimmed]);
  await loudnessMatch(trimmed, outFile, { target: -19, wav: true });
  await fs.unlink(trimmed).catch(() => {});
}

async function makeSilenceWav(outFile: string, seconds: number): Promise<void> {
  await ffmpeg([
    "-f",
    "lavfi",
    "-i",
    "anullsrc=r=44100:cl=mono",
    "-t",
    String(seconds),
    outFile,
  ]);
}

/** Concat WAVs (already same format) via the concat demuxer into one WAV. */
async function concatWavs(files: string[], outFile: string): Promise<void> {
  const listPath = path.join(os.tmpdir(), `bakeoff_list_${Date.now()}.txt`);
  await fs.writeFile(
    listPath,
    files.map((f) => `file '${f.replace(/'/g, "'\\''")}'`).join("\n"),
  );
  try {
    await ffmpeg(["-f", "concat", "-safe", "0", "-i", listPath, "-c", "copy", outFile]);
  } finally {
    await fs.unlink(listPath).catch(() => {});
  }
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
async function synthVoxtral(text: string, outDir: string): Promise<SynthResult> {
  const start = Date.now();
  const client = new Mistral({ apiKey: config.MISTRAL_API_KEY });
  const response = await client.audio.speech.complete(
    {
      model: VOXTRAL_MODEL,
      input: text,
      voiceId: VOXTRAL_VOICE.id,
      responseFormat: "mp3",
      stream: false,
    },
    { timeoutMs: 15 * 60 * 1000 },
  );
  const rawFile = path.join(outDir, "voxtral-raw.mp3");
  await fs.writeFile(rawFile, Buffer.from(response.audioData, "base64"));
  return {
    provider: "voxtral",
    rawFile,
    seconds: (Date.now() - start) / 1000,
    notes: [`voice=${VOXTRAL_VOICE.name}`, `model=${VOXTRAL_MODEL}`],
  };
}

async function synthEleven(
  text: string,
  outDir: string,
  variant: "eleven" | "eleven-tagged",
): Promise<SynthResult> {
  const apiKey = process.env.ELEVENLABS_API_KEY;
  if (!apiKey) throw new Error("ELEVENLABS_API_KEY not set");
  const start = Date.now();
  const chunks = chunkText(text, ELEVEN_CHUNK_TARGET, ELEVEN_CHUNK_MAX);
  logger.info(`[${variant}] synthesizing ${chunks.length} chunks`);

  const wavs: string[] = [];
  const gapWav = path.join(os.tmpdir(), `bakeoff_gap_${Date.now()}.wav`);
  await makeSilenceWav(gapWav, 0.75);

  for (let i = 0; i < chunks.length; i++) {
    const body = {
      text: chunks[i],
      model_id: "eleven_v3",
      seed: ELEVEN_SEED,
      // v3 stability: 0.0 Creative / 0.5 Natural / 1.0 Robust — Natural mode
      voice_settings: { stability: 0.5, use_speaker_boost: true },
    };
    const audio = await got
      .post(
        `https://api.elevenlabs.io/v1/text-to-speech/${ELEVEN_VOICE_ID}?output_format=mp3_44100_128`,
        {
          headers: { "xi-api-key": apiKey },
          json: body,
          timeout: { request: 5 * 60 * 1000 },
        },
      )
      .buffer();
    const mp3 = path.join(os.tmpdir(), `bakeoff_el_${Date.now()}_${i}.mp3`);
    const wav = mp3.replace(/\.mp3$/, ".wav");
    await fs.writeFile(mp3, audio);
    await prepareChunkWav(mp3, wav);
    await fs.unlink(mp3).catch(() => {});
    if (i > 0) wavs.push(gapWav);
    wavs.push(wav);
    logger.info(`[${variant}] chunk ${i + 1}/${chunks.length} done`);
  }

  const joined = path.join(os.tmpdir(), `bakeoff_el_join_${Date.now()}.wav`);
  await concatWavs(wavs, joined);
  const rawFile = path.join(outDir, `${variant}-raw.mp3`);
  await ffmpeg(["-i", joined, "-c:a", "libmp3lame", "-b:a", "128k", rawFile]);
  for (const f of [...new Set(wavs), joined]) await fs.unlink(f).catch(() => {});

  return {
    provider: variant,
    rawFile,
    seconds: (Date.now() - start) / 1000,
    notes: [
      `voice=${ELEVEN_VOICE_ID}`,
      `model=eleven_v3 (Natural, seed=${ELEVEN_SEED})`,
      `chunks=${chunks.length} @ ~${ELEVEN_CHUNK_TARGET} chars, per-chunk -19 LUFS, 12ms edge fades, 0.75s gaps`,
    ],
  };
}

async function synthMinimax(text: string, outDir: string): Promise<SynthResult> {
  const apiKey = process.env.MINIMAX_API_KEY;
  const groupId = process.env.MINIMAX_GROUP_ID;
  if (!apiKey || !groupId)
    throw new Error("MINIMAX_API_KEY / MINIMAX_GROUP_ID not set");
  const start = Date.now();
  const headers = { Authorization: `Bearer ${apiKey}` };

  const createRes = await got
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
    })
    .json<Record<string, unknown>>();
  logger.info("[minimax] task created", { createRes });
  const baseResp = createRes.base_resp as
    | { status_code?: number; status_msg?: string }
    | undefined;
  if (baseResp && baseResp.status_code !== 0) {
    throw new Error(
      `minimax create failed (${baseResp.status_code}): ${baseResp.status_msg}`,
    );
  }
  const taskId =
    (createRes.task_id as string | number | undefined) ??
    ((createRes.data as Record<string, unknown> | undefined)?.task_id as
      | string
      | number
      | undefined);
  if (taskId === undefined || taskId === 0) {
    throw new Error(`minimax: no task_id in response: ${JSON.stringify(createRes)}`);
  }

  // Poll up to 30 minutes
  let fileId: string | number | undefined;
  for (let attempt = 0; attempt < 180; attempt++) {
    await new Promise((r) => setTimeout(r, 10_000));
    const q = await got
      .get(
        `https://api.minimax.io/v1/query/t2a_async_query_v2?GroupId=${groupId}&task_id=${taskId}`,
        { headers, timeout: { request: 60_000 } },
      )
      .json<Record<string, unknown>>();
    const status = String(
      q.status ?? (q.data as Record<string, unknown>)?.status ?? "",
    );
    if (attempt % 6 === 0) logger.info(`[minimax] poll status=${status || "?"}`, { q });
    if (/success/i.test(status)) {
      fileId =
        (q.file_id as string | number | undefined) ??
        ((q.data as Record<string, unknown> | undefined)?.file_id as
          | string
          | number
          | undefined);
      break;
    }
    if (/fail|expired/i.test(status)) {
      throw new Error(`minimax task failed: ${JSON.stringify(q)}`);
    }
  }
  if (fileId === undefined) throw new Error("minimax: timed out waiting for task");

  const fileRes = await got.get(
    `https://api.minimax.io/v1/files/retrieve_content?GroupId=${groupId}&file_id=${fileId}`,
    { headers, timeout: { request: 5 * 60 * 1000 } },
  );
  let audio: Uint8Array = fileRes.rawBody;
  // Some deployments return JSON metadata with a download_url instead of bytes
  if (fileRes.headers["content-type"]?.includes("application/json")) {
    const meta = JSON.parse(fileRes.rawBody.toString());
    const url = meta.file?.download_url ?? meta.download_url ?? meta.data?.download_url;
    if (!url) throw new Error(`minimax: no download_url: ${fileRes.rawBody}`);
    audio = await got.get(url, { timeout: { request: 5 * 60 * 1000 } }).buffer();
  }

  const rawFile = path.join(outDir, "minimax-raw.mp3");
  await fs.writeFile(rawFile, audio);
  return {
    provider: "minimax",
    rawFile,
    seconds: (Date.now() - start) / 1000,
    notes: [`voice=${MINIMAX_VOICE_ID}`, "model=speech-2.8-hd (async, single request)"],
  };
}

async function synthFish(text: string, outDir: string): Promise<SynthResult> {
  const apiKey = process.env.FISH_API_KEY;
  if (!apiKey) throw new Error("FISH_API_KEY not set");
  const start = Date.now();
  const referenceId = FISH_REFERENCE_ID;
  const audio = await got
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
    })
    .buffer();
  const rawFile = path.join(outDir, "fish-raw.mp3");
  await fs.writeFile(rawFile, audio);
  return {
    provider: "fish",
    rawFile,
    seconds: (Date.now() - start) / 1000,
    notes: [
      `voice=${referenceId}`,
      `model=${FISH_MODEL} (single request, vendor chunking)`,
    ],
  };
}

// ---------------------------------------------------------------------------
// Optional: LLM "director" pass adding ElevenLabs v3 audio tags
// ---------------------------------------------------------------------------
async function addAudioTags(text: string): Promise<string> {
  const { model } = getPressPodsCleaningModel();
  const { text: out } = await generateText({
    model,
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
  });
  return out.trim();
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

async function runProviders({
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
}): Promise<void> {
  await fs.mkdir(outDir, { recursive: true });
  await fs.writeFile(path.join(outDir, "narration.md"), content);

  let taggedContent: string | undefined;
  if (tagged && providers.includes("eleven")) {
    taggedContent = await addAudioTags(content);
    await fs.writeFile(path.join(outDir, "narration-tagged.md"), taggedContent);
    logger.info("Tagged narration variant ready");
  }

  const jobs: Array<() => Promise<SynthResult>> = [];
  if (providers.includes("voxtral")) jobs.push(() => synthVoxtral(content, outDir));
  if (providers.includes("eleven"))
    jobs.push(() => synthEleven(content, outDir, "eleven"));
  if (taggedContent) {
    const t = taggedContent;
    jobs.push(() => synthEleven(t, outDir, "eleven-tagged"));
  }
  if (providers.includes("minimax")) jobs.push(() => synthMinimax(content, outDir));
  if (providers.includes("fish")) jobs.push(() => synthFish(content, outDir));

  const settled = await Promise.allSettled(jobs.map((j) => j()));
  const results: SynthResult[] = [];
  for (const s of settled) {
    if (s.status === "fulfilled") results.push(s.value);
    else logger.error(`Provider failed: ${s.reason}`);
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
    try {
      await loudnessMatch(r.rawFile, outFile);
      const dur = await probeDuration(outFile);
      lines.push(
        `| ${r.provider} | ${(dur / 60).toFixed(1)} min | ${r.seconds.toFixed(0)}s | ${r.notes.join("; ")} |`,
      );
    } catch (error) {
      logger.error(`Loudness match failed for ${r.provider}: ${error}`);
      lines.push(
        `| ${r.provider} | ? | ${r.seconds.toFixed(0)}s | loudnorm FAILED, use ${path.basename(r.rawFile)} |`,
      );
    }
  }
  const summary = lines.join("\n");
  const summaryPath = path.join(outDir, "summary.md");
  const existing = await fs.readFile(summaryPath, "utf8").catch(() => undefined);
  await fs.writeFile(
    summaryPath,
    existing ? `${existing}\n\n## Rerun\n\n${summary}` : summary,
  );
  logger.info(`\n${summary}\n\nOutput: ${outDir}`);
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const urls = args.filter((a) => !a.startsWith("--"));
  const providersFlag = args.find((a) => a.startsWith("--providers="));
  const outFlag = args.find((a) => a.startsWith("--out="));
  const narrationFlag = args.find((a) => a.startsWith("--narration="));
  const tagged = args.includes("--tagged");
  if (urls.length === 0 && !narrationFlag) {
    console.error(
      "Usage: bun src/tools/tts-bakeoff.ts <article-url> [...urls] [--providers=...] [--tagged] [--out=dir]\n" +
        "   or: bun src/tools/tts-bakeoff.ts --narration=path/to/narration.md [--providers=...]",
    );
    process.exit(1);
  }

  const providers = providersFlag
    ? (providersFlag.split("=")[1].split(",") as ProviderName[])
    : detectProviders();
  const outRoot =
    outFlag?.split("=")[1] ?? path.join(os.homedir(), "Documents", "tts-bakeoff");
  logger.info(`Providers: ${providers.join(", ")}${tagged ? " + eleven-tagged" : ""}`);

  // Re-run providers against an existing narration file (identical text, so
  // late entrants stay comparable with earlier outputs in the same directory).
  if (narrationFlag) {
    const narrationPath = narrationFlag.split("=")[1];
    const content = await fs.readFile(narrationPath, "utf8");
    const outDir = path.dirname(path.resolve(narrationPath));
    const title = path.basename(outDir);
    await runProviders({ content, outDir, title, providers, tagged });
    return;
  }

  for (const url of urls) {
    logger.info(`=== Article: ${url}`);
    const costCounter = new CostCounter();
    const { article, metadata } = await getArticleFromUrl(url, costCounter, logger);
    const title = metadata.info.title ?? article.title ?? url;
    const text = buildFinalText({
      title,
      domain: metadata.info.publication ?? article.domain,
      author: metadata.info.author ?? article.author ?? "Anonymous",
      coauthors: metadata.info.coauthors,
      datePublished: metadata.info.publishedAtISO ?? article.publishedAt,
      text: article.text,
    });
    const { content } = await getCleanedArticle({ ...article, text }, costCounter);
    logger.info(`Narration ready: ${content.length} chars`);

    await runProviders({
      content,
      outDir: path.join(outRoot, slugify(title)),
      title,
      url,
      providers,
      tagged,
    });
  }
}

await main();
