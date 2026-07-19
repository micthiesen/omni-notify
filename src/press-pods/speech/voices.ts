import config from "../../utils/config.js";
import type { MetadataInfo } from "../agents/metadata.js";

export interface Voice {
  id: string;
  name: string;
}

/**
 * Curated ElevenLabs premade narrator voices. The active pick per gender is
 * index 0; the rest are audition candidates — swap by reordering, or set
 * ELEVENLABS_VOICE_MALE / ELEVENLABS_VOICE_FEMALE to any voice id.
 *
 * Audition every voice in the ElevenLabs voice library (search by name):
 *   https://elevenlabs.io/app/voice-library
 * These are the long-stable classic premade ids; confirm by ear before
 * promoting one to the default.
 */
export const NARRATOR_VOICES: { male: Voice[]; female: Voice[] } = {
  male: [
    { id: "nPczCjzI2devNBz1zQrb", name: "Brian" }, // deep American, warm narration
    { id: "JBFqnCBsd6RMkjVDRZzb", name: "George" }, // British, warm storyteller
    { id: "onwK4e9ZLuTAKqWW03F9", name: "Daniel" }, // British, authoritative news
    { id: "pqHfZKP75CvOlQylNhV4", name: "Bill" }, // older, documentary
    { id: "pNInz6obpgDQGcFmaJgB", name: "Adam" }, // deep, classic narration
  ],
  female: [
    { id: "XrExE9yKIg1WjnnlVkGX", name: "Matilda" }, // warm American, narration
    { id: "Xb7hH8MSUJpSbSDYk0k2", name: "Alice" }, // British, clear, news
    { id: "pFZP5JQG7iQjIQuC4Bku", name: "Lily" }, // British, warm narration
    { id: "EXAVITQu4vr4xnSDxMaL", name: "Sarah" }, // American, soft, professional
    { id: "21m00Tcm4TlvDq8ikWAM", name: "Rachel" }, // American, calm, classic
  ],
};

/**
 * Pick the narrator voice. Author gender selects the family (female/unknown →
 * female, matching the old Voxtral behavior); an ELEVENLABS_VOICE_* override
 * wins outright.
 */
export function getVoice(authorGender: MetadataInfo["authorGender"]): Voice {
  if (authorGender === "male") {
    return config.ELEVENLABS_VOICE_MALE
      ? resolveOverride(config.ELEVENLABS_VOICE_MALE, NARRATOR_VOICES.male)
      : NARRATOR_VOICES.male[0];
  }
  return config.ELEVENLABS_VOICE_FEMALE
    ? resolveOverride(config.ELEVENLABS_VOICE_FEMALE, NARRATOR_VOICES.female)
    : NARRATOR_VOICES.female[0];
}

/** Resolve an override id to a known name when possible, else label it custom. */
function resolveOverride(id: string, known: Voice[]): Voice {
  return known.find((v) => v.id === id) ?? { id, name: "Custom" };
}
