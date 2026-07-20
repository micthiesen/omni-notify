import type { Logger } from "@micthiesen/mitools/logging";
import type { MetadataInfo } from "../../agents/metadata.js";

export type AuthorGender = MetadataInfo["authorGender"];

/**
 * A TTS backend. `synthesizeChunk` turns one narration chunk into MP3 bytes;
 * chunking, per-chunk mastering, and stitching are provider-agnostic (see
 * synthesize.ts). `needsDenoise` gates the audio-chain denoise pass — on for
 * self-hosted models with a noise floor, off for clean cloud output.
 */
export interface TtsProvider {
  readonly providerName: string;
  readonly voiceName: string;
  readonly modelId: string;
  readonly needsDenoise: boolean;
  /**
   * Whether to length-verify each chunk and retry on truncation/runaway. Local
   * autoregressive models (Higgs) emit a wildly variable amount of audio for
   * the same text; cloud models (ElevenLabs) are reliable and skip the check.
   */
  readonly verifyChunkLength: boolean;
  /**
   * Whether to STT-transcribe each chunk and reject on missing content. Higgs
   * truncates mid-chunk in a way the duration check can't reliably catch (a
   * truncated read and a fast read overlap in seconds/char); word coverage
   * separates them. On for Higgs, off for reliable cloud models. Needs an STT
   * endpoint (createSttClient) — degrades to the duration check without one.
   */
  readonly verifyChunkContent: boolean;
  synthesizeChunk(text: string, logger: Logger): Promise<Buffer>;
}
