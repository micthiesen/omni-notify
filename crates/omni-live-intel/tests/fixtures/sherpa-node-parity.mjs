// Reference side of the sherpa parity spike (`tests/sherpa_parity.rs`).
//
// Runs the TS speech path (`src/live-check/intelligence/localSpeech.ts`) with
// the repository's sherpa-onnx-node addon over 16 kHz mono f32le clips, and
// writes each clip's VAD segment lengths, 4 s / 2 s-stride window embeddings
// and `detectDestinyEffect` result against a voiceprint enrolled from the
// first clip's even windows.
//
//   node sherpa-node-parity.mjs MODEL_DIR CLIP_DIR OUT.json NAME [NAME...]
//
// MODEL_DIR holds silero_vad.int8.onnx and
// 3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx (the Docker image files);
// CLIP_DIR holds NAME.f32 for every NAME.
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";

const require = createRequire(new URL("../../../../package.json", import.meta.url));
const sherpa = require("sherpa-onnx-node");
const [modelDir, clipDir, out, ...names] = process.argv.slice(2);
if (!modelDir || !clipDir || !out || names.length < 2) {
  throw new Error("Usage: MODEL_DIR CLIP_DIR OUT.json NAME [NAME...]");
}
const SAMPLE_RATE = 16_000;
const WINDOW = 4 * SAMPLE_RATE;
const STRIDE = 2 * SAMPLE_RATE;
const THRESHOLD = 0.62;

const extractor = new sherpa.SpeakerEmbeddingExtractor({
  model: join(modelDir, "3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx"),
  numThreads: 1,
  provider: "cpu",
  debug: 0,
});

function speechSegments(samples) {
  const vad = new sherpa.Vad(
    {
      sileroVad: {
        model: join(modelDir, "silero_vad.int8.onnx"),
        threshold: 0.5,
        minSpeechDuration: 0.25,
        minSilenceDuration: 0.35,
        windowSize: 512,
        maxSpeechDuration: 20,
      },
      sampleRate: SAMPLE_RATE,
      numThreads: 1,
      provider: "cpu",
      debug: 0,
    },
    120,
  );
  for (let offset = 0; offset < samples.length; offset += 512) {
    vad.acceptWaveform(samples.subarray(offset, Math.min(samples.length, offset + 512)));
  }
  vad.flush();
  const segments = [];
  while (!vad.isEmpty()) {
    const segment = vad.front(false);
    if (segment.samples.length >= SAMPLE_RATE) segments.push(segment.samples);
    vad.pop();
  }
  return segments;
}

function embedding(samples) {
  const stream = extractor.createStream();
  stream.acceptWaveform({ sampleRate: SAMPLE_RATE, samples });
  if (!extractor.isReady(stream)) throw new Error("Speaker sample is too short");
  return extractor.compute(stream, false);
}

function cosineSimilarity(a, b) {
  if (a.length !== b.length || a.length === 0) return -1;
  let dot = 0;
  let normA = 0;
  let normB = 0;
  for (let index = 0; index < a.length; index += 1) {
    dot += a[index] * b[index];
    normA += a[index] * a[index];
    normB += b[index] * b[index];
  }
  return dot / Math.max(Number.EPSILON, Math.sqrt(normA) * Math.sqrt(normB));
}

const clips = {};
for (const name of names) {
  const bytes = readFileSync(join(clipDir, `${name}.f32`));
  const aligned = Uint8Array.from(bytes.subarray(0, bytes.length - (bytes.length % 4)));
  const segments = speechSegments(new Float32Array(aligned.buffer));
  const windows = [];
  for (const segment of segments) {
    for (let offset = 0; offset + WINDOW <= segment.length; offset += STRIDE) {
      windows.push(Array.from(embedding(segment.subarray(offset, offset + WINDOW))));
    }
  }
  clips[name] = { segmentLengths: segments.map((segment) => segment.length), windows };
}

const voiceprint = {
  version: 1,
  speaker: "destiny",
  model: "3dspeaker-campplus-en-voxceleb-16k",
  embeddings: clips[names[0]].windows.filter((_, index) => index % 2 === 0).slice(0, 6),
  createdAt: 1,
  sources: [`clip:${names[0]}`],
};
for (const name of names) {
  const scores = clips[name].windows
    .map((window) =>
      Math.max(
        ...voiceprint.embeddings.map((reference) =>
          cosineSimilarity(Float32Array.from(window), reference),
        ),
      ),
    )
    .sort((a, b) => b - a);
  clips[name].detect = {
    confidence: scores[0] ?? 0,
    matchedWindows: scores.filter((score) => score >= THRESHOLD).length,
    checkedWindows: scores.length,
  };
}
writeFileSync(out, JSON.stringify({ names, voiceprint, clips }));
for (const name of names) {
  console.log(name, JSON.stringify(clips[name].detect));
}
