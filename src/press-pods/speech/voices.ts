import type { MetadataInfo } from "../agents/metadata.js";

export interface Voice {
  id: string;
  name: string;
}

export interface VoiceChoices {
  male: Voice[];
  female: Voice[];
}

/**
 * Preset voices from the Mistral API (`GET /v1/audio/voices`). Mistral's
 * preset catalog is thin; both gender slots currently hold the same two male
 * voices, so gender-aware selection is a no-op until distinct female presets
 * are slotted in (list them with:
 * `curl -s https://api.mistral.ai/v1/audio/voices -H "Authorization: Bearer $MISTRAL_API_KEY"`).
 */
export const VOICE_CHOICES: VoiceChoices = {
  male: [
    { id: "c69964a6-ab8b-4f8a-9465-ec0925096ec8", name: "Paul - Neutral" },
    { id: "e3596645-b1af-469e-b857-f18ddedc7652", name: "Oliver - Neutral" },
  ],
  female: [
    { id: "c69964a6-ab8b-4f8a-9465-ec0925096ec8", name: "Paul - Neutral" },
    { id: "e3596645-b1af-469e-b857-f18ddedc7652", name: "Oliver - Neutral" },
  ],
};

export function getRandomVoice(
  choices: VoiceChoices,
  authorGender: MetadataInfo["authorGender"],
): Voice {
  const voices = authorGender === "male" ? choices.male : choices.female;
  const randomIndex = Math.floor(Math.random() * voices.length);
  return voices[randomIndex];
}
