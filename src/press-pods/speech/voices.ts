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
 * Preset voices from the Mistral API (list with
 * `curl -s "https://api.mistral.ai/v1/audio/voices?limit=30" -H "Authorization: Bearer $MISTRAL_API_KEY"`).
 * Only the neutral-mood English presets are used: Paul (en_us) and Oliver
 * (en_gb) for male authors, Jane (en_gb) — the sole English female preset —
 * for female/unknown authors.
 */
export const VOICE_CHOICES: VoiceChoices = {
  male: [
    { id: "c69964a6-ab8b-4f8a-9465-ec0925096ec8", name: "Paul - Neutral" },
    { id: "e3596645-b1af-469e-b857-f18ddedc7652", name: "Oliver - Neutral" },
  ],
  female: [{ id: "82c99ee6-f932-423f-a4a3-d403c8914b8d", name: "Jane - Neutral" }],
};

export function getRandomVoice(
  choices: VoiceChoices,
  authorGender: MetadataInfo["authorGender"],
): Voice {
  const voices = authorGender === "male" ? choices.male : choices.female;
  const randomIndex = Math.floor(Math.random() * voices.length);
  return voices[randomIndex];
}
