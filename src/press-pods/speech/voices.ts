import config from "../../utils/config.js";
import type { MetadataInfo } from "../agents/metadata.js";

export interface Voice {
  id: string;
  name: string;
}

/**
 * ElevenLabs premade narrator voices. Author gender picks the family (matching
 * the old Voxtral behavior); female/unknown authors get the female voice. Both
 * ids are overridable via ELEVENLABS_VOICE_MALE / ELEVENLABS_VOICE_FEMALE —
 * audition alternatives in the ElevenLabs voice library and swap by id.
 */
const DEFAULT_MALE: Voice = { id: "nPczCjzI2devNBz1zQrb", name: "Brian" };
const DEFAULT_FEMALE: Voice = { id: "XrExE9yKIg1WjnnlVkGX", name: "Matilda" };

export function getVoice(authorGender: MetadataInfo["authorGender"]): Voice {
  if (authorGender === "male") {
    return config.ELEVENLABS_VOICE_MALE
      ? { id: config.ELEVENLABS_VOICE_MALE, name: "Custom (male)" }
      : DEFAULT_MALE;
  }
  return config.ELEVENLABS_VOICE_FEMALE
    ? { id: config.ELEVENLABS_VOICE_FEMALE, name: "Custom (female)" }
    : DEFAULT_FEMALE;
}
