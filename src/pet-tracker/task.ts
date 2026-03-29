import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";

import config, { type Config } from "../utils/config.js";
import { fetchPetsByUser } from "./api.js";
import { authenticateWhisker } from "./auth.js";
import { linearRegression } from "./math.js";
import {
  getRecentWeightHistory,
  insertWeightReading,
  upsertPet,
} from "./persistence.js";

type Credentials = NonNullable<Config["WHISKER_CREDENTIALS"]>;

const MS_PER_DAY = 86_400_000;

function round(value: number, decimals: number): string {
  const factor = 10 ** decimals;
  return (Math.round(value * factor) / factor).toFixed(decimals);
}

interface PetSyncResult {
  petId: string;
  name: string;
  currentWeight: number;
}

export default class PetTrackerTask extends ScheduledTask {
  public readonly name = "PetTracker";
  public readonly schedule = "0 */10 * * * *";
  public readonly runOnStartup = true;

  private readonly logger: Logger;
  private readonly credentials: Credentials;

  constructor(credentials: Credentials, logger: Logger) {
    super();
    this.credentials = credentials;
    this.logger = logger.extend("PetTracker");
  }

  public async run(): Promise<void> {
    const { idToken, userId } = await authenticateWhisker(
      this.credentials.email,
      this.credentials.password,
    );

    const pets = await fetchPetsByUser(idToken, userId);

    const affectedPets: PetSyncResult[] = [];
    let totalNew = 0;
    const now = new Date().toISOString();

    for (const pet of pets) {
      upsertPet({
        pet_id: pet.petId,
        name: pet.name,
        current_weight: pet.weight,
        updated_at: now,
      });

      let newReadings = 0;
      for (const reading of pet.weightHistory) {
        const isNew = insertWeightReading({
          pet_id: pet.petId,
          timestamp: reading.timestamp,
          weight: reading.weight,
        });
        if (isNew) newReadings++;
      }
      totalNew += newReadings;

      if (newReadings > 0) {
        affectedPets.push({
          petId: pet.petId,
          name: pet.name,
          currentWeight: pet.weight,
        });
      }
    }

    const syncMsg = `Synced ${pets.length} pets, ${totalNew} new / ${pets.reduce((s, p) => s + p.weightHistory.length, 0)} total readings`;
    this.logger[totalNew > 0 ? "info" : "debug"](syncMsg);

    if (affectedPets.length > 0) {
      const title = formatTitle(affectedPets.map((p) => p.name));
      const message = affectedPets.map((p) => formatPetLine(p)).join("\n");

      await notify({
        title,
        message,
        token: config.PUSHOVER_TOKEN,
      });
    }
  }
}

function formatTitle(names: string[]): string {
  if (names.length === 1) return `${names[0]} Weighed In`;
  if (names.length === 2) return `${names[0]} & ${names[1]} Weighed In`;
  const last = names.pop()!;
  return `${names.join(", ")} & ${last} Weighed In`;
}

function formatPetLine(pet: PetSyncResult): string {
  const history = getRecentWeightHistory(pet.petId, 30);
  const weight = `${round(pet.currentWeight, 1)} lbs`;

  if (history.length < 2) return `${pet.name}: ${weight}`;

  const t0 = new Date(history[0].timestamp).getTime();
  const points = history.map((h) => ({
    x: (new Date(h.timestamp).getTime() - t0) / MS_PER_DAY,
    y: h.weight,
  }));
  const { slope, r2 } = linearRegression(points);
  const perWeek = slope * 7;

  const sign = perWeek >= 0 ? "+" : "";
  const trend = `${sign}${round(perWeek, 2)} lbs/wk`;
  const qualifier = r2 < 0.3 ? ", weak trend" : "";

  return `${pet.name}: ${weight} (${trend}${qualifier})`;
}
