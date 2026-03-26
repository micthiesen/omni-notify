import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";

import config, { type Config } from "../utils/config.js";
import { fetchPetsByUser } from "./api.js";
import { authenticateWhisker } from "./auth.js";
import { insertWeightReading, upsertPet } from "./persistence.js";

type Credentials = NonNullable<Config["WHISKER_CREDENTIALS"]>;

export default class PetTrackerTask extends ScheduledTask {
  public readonly name = "PetTracker";
  public readonly schedule = "0 0 */2 * * *";
  public readonly runOnStartup = true;

  private readonly logger: Logger;
  private readonly credentials: Credentials;

  constructor(credentials: Credentials, logger: Logger) {
    super();
    this.credentials = credentials;
    this.logger = logger;
  }

  public async run(): Promise<void> {
    const { idToken, userId } = await authenticateWhisker(
      this.credentials.email,
      this.credentials.password,
    );

    const pets = await fetchPetsByUser(idToken, userId);

    let newReadings = 0;
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
        const isNew = insertWeightReading({
          pet_id: pet.petId,
          timestamp: reading.timestamp,
          weight: reading.weight,
        });
        if (isNew) newReadings++;
        totalReadings++;
      }
    }

    this.logger.info(
      `Synced ${pets.length} pets, ${newReadings} new / ${totalReadings} total readings`,
    );

    if (newReadings > 0) {
      await notify({
        title: "Pet Tracker",
        message: `${newReadings} new weight reading${newReadings === 1 ? "" : "s"} from ${pets.length} pet${pets.length === 1 ? "" : "s"}`,
        token: config.PUSHOVER_TOKEN,
      });
    }
  }
}
