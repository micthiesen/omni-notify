import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";

import config from "../utils/config.js";
import { fetchPetsByUser } from "./api.js";
import { authenticateWhisker } from "./auth.js";
import { insertWeightReading, upsertPet } from "./persistence.js";

export default class PetTrackerTask extends ScheduledTask {
  public readonly name = "PetTracker";
  public readonly schedule = "0 0 */2 * * *";
  public readonly runOnStartup = true;

  private readonly logger: Logger;

  constructor(logger: Logger) {
    super();
    this.logger = logger;
  }

  public async run(): Promise<void> {
    const credentials = config.WHISKER_CREDENTIALS;
    if (!credentials) {
      this.logger.info("WHISKER_CREDENTIALS not set, skipping pet sync");
      return;
    }

    const { idToken, userId } = await authenticateWhisker(
      credentials.email,
      credentials.password,
    );

    const pets = await fetchPetsByUser(idToken, userId);

    let totalReadings = 0;
    const now = new Date().toISOString();

    for (const pet of pets) {
      upsertPet({
        pet_id: pet.petId,
        name: pet.name,
        current_weight: pet.weight,
        updated_at: now,
      });

      for (const reading of pet.weightHistory) {
        insertWeightReading({
          pet_id: pet.petId,
          timestamp: reading.timestamp,
          weight: reading.weight,
        });
        totalReadings++;
      }
    }

    this.logger.info(`Synced ${pets.length} pets, ${totalReadings} weight readings`);
  }
}
