import config from "../../utils/config.js";
import type { MetadataInfo } from "../agents/metadata.js";

export interface Voice {
  id: string;
  name: string;
}

/**
 * The chosen ElevenLabs narrator voice per author gender (female/unknown →
 * female, matching the old Voxtral behavior). To try another voice, audition
 * in the library (https://elevenlabs.io/app/voice-library) and set its id via
 * ELEVENLABS_VOICE_MALE / ELEVENLABS_VOICE_FEMALE.
 */
const MALE_VOICE: Voice = { id: "nPczCjzI2devNBz1zQrb", name: "Brian" };
const FEMALE_VOICE: Voice = { id: "XrExE9yKIg1WjnnlVkGX", name: "Matilda" };

export function getVoice(authorGender: MetadataInfo["authorGender"]): Voice {
  if (authorGender === "male") {
    return config.ELEVENLABS_VOICE_MALE
      ? { id: config.ELEVENLABS_VOICE_MALE, name: "Custom" }
      : MALE_VOICE;
  }
  return config.ELEVENLABS_VOICE_FEMALE
    ? { id: config.ELEVENLABS_VOICE_FEMALE, name: "Custom" }
    : FEMALE_VOICE;
}
