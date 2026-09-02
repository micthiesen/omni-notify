import { Logger, LogLevel } from "@micthiesen/mitools/logging";
import { Sqlite } from "@micthiesen/mitools/sqlite";
import { Effect, Layer } from "effect";

const { clearAllData, insertWeightReading, upsertPet } =
  await import("./persistence.js");

interface PetSeed {
  petId: string;
  name: string;
  baselineWeight: number;
  trendPerDay: number;
}

const pets: PetSeed[] = [
  {
    petId: "seed-luna",
    name: "Luna",
    baselineWeight: 9.5,
    trendPerDay: 0.5 / 90,
  },
  {
    petId: "seed-mochi",
    name: "Mochi",
    baselineWeight: 11.2,
    trendPerDay: -0.3 / 90,
  },
  {
    petId: "seed-pepper",
    name: "Pepper",
    baselineWeight: 7.8,
    trendPerDay: 0.2 / 90,
  },
];

const DAYS = 90;
const MIN_READINGS_PER_DAY = 3;
const MAX_READINGS_PER_DAY = 5;
const FLUCTUATION = 0.3;

function randomInt(min: number, max: number): number {
  return Math.floor(Math.random() * (max - min + 1)) + min;
}

function randomFloat(min: number, max: number): number {
  return Math.random() * (max - min) + min;
}

function generateReadingTimes(day: Date, count: number): Date[] {
  const times: Date[] = [];
  for (let i = 0; i < count; i++) {
    const hour = randomInt(6, 23);
    const minute = randomInt(0, 59);
    const second = randomInt(0, 59);
    const ts = new Date(day);
    ts.setHours(hour, minute, second, 0);
    times.push(ts);
  }
  return times.sort((a, b) => a.getTime() - b.getTime());
}

const seed = Effect.gen(function* () {
  yield* clearAllData();

  const now = new Date();
  let totalReadings = 0;

  for (const pet of pets) {
    const finalWeight = pet.baselineWeight + pet.trendPerDay * DAYS;

    for (let dayOffset = DAYS - 1; dayOffset >= 0; dayOffset--) {
      const day = new Date(now);
      day.setDate(day.getDate() - dayOffset);
      day.setHours(0, 0, 0, 0);

      const dayIndex = DAYS - dayOffset;
      const trendWeight = pet.baselineWeight + pet.trendPerDay * dayIndex;
      const readingCount = randomInt(MIN_READINGS_PER_DAY, MAX_READINGS_PER_DAY);
      const times = generateReadingTimes(day, readingCount);

      for (const ts of times) {
        const fluctuation = randomFloat(-FLUCTUATION, FLUCTUATION);
        const weight = Math.round((trendWeight + fluctuation) * 100) / 100;

        yield* insertWeightReading({
          pet_id: pet.petId,
          timestamp: ts.toISOString(),
          weight,
        });
        totalReadings++;
      }
    }

    yield* upsertPet({
      pet_id: pet.petId,
      name: pet.name,
      current_weight: Math.round(finalWeight * 100) / 100,
      updated_at: now.toISOString(),
    });
  }

  console.log(
    `Seeded ${pets.length} pets with ${totalReadings} weight readings (${DAYS} days)`,
  );
});

await Effect.runPromise(
  seed.pipe(
    Effect.provide(
      Layer.merge(
        Sqlite.layer({ path: "docstore.db" }),
        Logger.layer({ level: LogLevel.INFO }),
      ),
    ),
  ),
);
