import { LivestreamAudioCapture } from "../live-check/intelligence/audio.js";
import { LocalSpeechRuntime } from "../live-check/intelligence/localSpeech.js";

function valueAfter(args: string[], flag: string): string | undefined {
  const index = args.indexOf(flag);
  return index >= 0 ? args[index + 1] : undefined;
}

const args = process.argv.slice(2);
const url = valueAfter(args, "--url");
if (!url) throw new Error("Usage: --url URL [--seek SECONDS] [--duration SECONDS]");
const duration = Number(valueAfter(args, "--duration") ?? 30);
const seek = Number(valueAfter(args, "--seek") ?? 0);
const modelDir =
  valueAfter(args, "--model-dir") ??
  process.env.LIVESTREAM_MODEL_DIR ??
  "/app/assets/livestream-intelligence/models";
const voiceprint =
  valueAfter(args, "--voiceprint") ?? process.env.LIVESTREAM_DESTINY_VOICEPRINT_PATH;

const startedAt = performance.now();
const capture = new LivestreamAudioCapture();
const audio = await capture.capture(url, duration, seek);
const capturedAt = performance.now();
const speech = new LocalSpeechRuntime(modelDir, voiceprint);
const transcript = await speech.transcribe(audio.samples);
const transcribedAt = performance.now();
const match = speech.detectDestiny(audio.samples);
const finishedAt = performance.now();

process.stdout.write(
  `${JSON.stringify(
    {
      audioSeconds: audio.durationSeconds,
      captureSeconds: (capturedAt - startedAt) / 1000,
      transcriptionSeconds: (transcribedAt - capturedAt) / 1000,
      transcriptionRealtimeFactor:
        (transcribedAt - capturedAt) / 1000 / audio.durationSeconds,
      speakerSeconds: (finishedAt - transcribedAt) / 1000,
      match,
      transcript,
    },
    null,
    2,
  )}\n`,
);
