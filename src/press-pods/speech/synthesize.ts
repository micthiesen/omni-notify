import fs from "node:fs";
import fsAsync from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import type { Logger } from "@micthiesen/mitools/logging";
import { Mistral } from "@mistralai/mistralai";
import ffmpeg from "fluent-ffmpeg";
import config from "../../utils/config.js";
import type { MetadataInfo } from "../agents/metadata.js";
import type CostCounter from "../costs.js";
import { getRandomVoice, VOICE_CHOICES } from "./voices.js";

export const TTS_MODEL = "voxtral-mini-tts-2603";

const INTRO_PATH = "assets/press-pods/intro.mp3";

export interface SynthesisResult {
  audio: Buffer;
  voiceName: string;
  voiceProvider: string;
  synthesizedSeconds: number;
}

export async function synthesizeSpeech({
  content,
  authorGender,
  logger,
  costCounter,
}: {
  content: string;
  authorGender: MetadataInfo["authorGender"];
  logger: Logger;
  costCounter: CostCounter;
}): Promise<SynthesisResult> {
  const start = Date.now();
  const voice = getRandomVoice(VOICE_CHOICES, authorGender);

  logger.info("Starting speech synthesis", {
    voice: voice.name,
    totalChars: content.length,
  });

  const client = new Mistral({ apiKey: config.MISTRAL_API_KEY });
  const response = await client.audio.speech.complete(
    {
      model: TTS_MODEL,
      input: content,
      voiceId: voice.id,
      responseFormat: "mp3",
      stream: false,
    },
    { timeoutMs: 10 * 60 * 1000 },
  );

  const ttsBuffer = Buffer.from(response.audioData, "base64");
  costCounter.recordTtsUsage(TTS_MODEL, "tts", content);
  logger.info("Speech synthesized", { ttsBytes: ttsBuffer.length });

  const introBuffer = await fsAsync.readFile(INTRO_PATH);
  const audio = await concatMp3WithLoudnorm(introBuffer, ttsBuffer);

  return {
    audio,
    voiceName: voice.name,
    voiceProvider: "Mistral",
    synthesizedSeconds: (Date.now() - start) / 1000,
  };
}

/** Normalize TTS audio loudness, then concatenate with the intro jingle. */
async function concatMp3WithLoudnorm(
  introBuffer: Buffer,
  ttsBuffer: Buffer,
): Promise<Buffer> {
  const tmpDir = os.tmpdir();
  const ts = Date.now();
  const ttsInPath = path.join(tmpDir, `tts_raw_${ts}.mp3`);
  const ttsNormPath = path.join(tmpDir, `tts_norm_${ts}.mp3`);
  const introPath = path.join(tmpDir, `tts_intro_${ts}.mp3`);
  const listPath = path.join(tmpDir, `tts_list_${ts}.txt`);
  const tempFiles = [ttsInPath, ttsNormPath, introPath, listPath];

  try {
    // 1. Loudnorm the TTS audio only
    fs.writeFileSync(ttsInPath, ttsBuffer);
    await new Promise<void>((resolve, reject) => {
      ffmpeg(ttsInPath)
        .audioFilter("loudnorm=I=-16:TP=-1.5:LRA=11")
        .audioCodec("libmp3lame")
        .audioBitrate("128k")
        .format("mp3")
        .on("error", reject)
        .on("end", () => resolve())
        .save(ttsNormPath);
    });

    // 2. Concat intro + normalized TTS (re-encode to common format)
    fs.writeFileSync(introPath, introBuffer);
    fs.writeFileSync(
      listPath,
      `file '${introPath.replace(/'/g, "'\\''")}'\nfile '${ttsNormPath.replace(/'/g, "'\\''")}'`,
    );

    const outChunks: Buffer[] = [];
    await new Promise<void>((resolve, reject) => {
      ffmpeg()
        .input(listPath)
        .inputOptions(["-f", "concat", "-safe", "0"])
        .audioCodec("libmp3lame")
        .audioBitrate("128k")
        .format("mp3")
        .on("error", reject)
        .on("end", () => resolve())
        .pipe()
        .on("data", (c: Buffer) => outChunks.push(c));
    });

    return Buffer.concat(outChunks);
  } finally {
    for (const p of tempFiles) {
      try {
        fs.unlinkSync(p);
      } catch {}
    }
  }
}
