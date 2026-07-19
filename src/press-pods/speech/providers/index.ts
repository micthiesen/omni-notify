import config from "../../../utils/config.js";
import { ElevenLabsProvider } from "./elevenlabs.js";
import { HiggsProvider } from "./higgs.js";
import type { AuthorGender, TtsProvider } from "./types.js";

export type { AuthorGender, TtsProvider } from "./types.js";

/** Build the configured TTS provider for an episode's author gender. */
export function createTtsProvider(authorGender: AuthorGender): TtsProvider {
  return config.PRESSPODS_TTS_PROVIDER === "elevenlabs"
    ? new ElevenLabsProvider(authorGender)
    : new HiggsProvider(authorGender);
}
