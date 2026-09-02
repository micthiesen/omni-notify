import { Table } from "@micthiesen/mitools/table";
import { Clock, Effect } from "effect";

export interface PetRow {
  pet_id: string;
  name: string;
  current_weight: number;
  updated_at: string;
}
export interface WeightHistoryRow {
  pet_id: string;
  timestamp: string;
  weight: number;
}

const petsTable = Table.make<PetRow>({
  name: "pets",
  columns: {
    pet_id: { type: "TEXT", primaryKey: true },
    name: { type: "TEXT", notNull: true },
    current_weight: { type: "REAL", notNull: true },
    updated_at: { type: "TEXT", notNull: true },
  },
});
const weightHistoryTable = Table.make<WeightHistoryRow>({
  name: "pet_weight_history",
  columns: {
    pet_id: { type: "TEXT", notNull: true, primaryKey: true },
    timestamp: { type: "TEXT", notNull: true, primaryKey: true },
    weight: { type: "REAL", notNull: true },
  },
  indexes: [{ columns: ["pet_id", "timestamp"] }],
});

export const upsertPet = Effect.fn("PetTracker.upsertPet")(function* (pet: PetRow) {
  yield* (yield* petsTable).upsert(pet);
});
export const insertWeightReading = Effect.fn("PetTracker.insertWeightReading")(
  function* (reading: WeightHistoryRow) {
    return yield* (yield* weightHistoryTable).insert(reading);
  },
);
export const getPet = Effect.fn("PetTracker.getPet")(function* (petId: string) {
  return (yield* (yield* petsTable).query("pet_id = ?", [petId]))[0];
});
export const getAllPets = Effect.fn("PetTracker.getAllPets")(function* () {
  return yield* (yield* petsTable).all();
});
export const getWeightHistory = Effect.fn("PetTracker.getWeightHistory")(function* (
  petId: string,
) {
  return yield* (yield* weightHistoryTable).query("pet_id = ? ORDER BY timestamp ASC", [
    petId,
  ]);
});
export const getRecentWeightHistory = Effect.fn("PetTracker.getRecentWeightHistory")(
  function* (petId: string, days: number) {
    const cutoff = new Date((yield* Clock.currentTimeMillis) - days * 86_400_000);
    return yield* (yield* weightHistoryTable).query(
      "pet_id = ? AND timestamp >= ? ORDER BY timestamp ASC",
      [petId, cutoff.toISOString()],
    );
  },
);
export const getAllPetsWithHistory = Effect.fn("PetTracker.getAllPetsWithHistory")(
  function* () {
    const pets = yield* getAllPets();
    return yield* Effect.forEach(pets, (pet) =>
      getWeightHistory(pet.pet_id).pipe(
        Effect.map((weightHistory) => ({ ...pet, weightHistory })),
      ),
    );
  },
);
export interface DailyVisitCount {
  date: string;
  count: number;
}
export const getDailyVisitCounts = Effect.fn("PetTracker.getDailyVisitCounts")(
  function* (petId: string) {
    const history = yield* getWeightHistory(petId);
    const counts = new Map<string, number>();
    for (const row of history) {
      const date = row.timestamp.slice(0, 10);
      counts.set(date, (counts.get(date) ?? 0) + 1);
    }
    return Array.from(counts.entries())
      .map(([date, count]) => ({ date, count }))
      .sort((a, b) => a.date.localeCompare(b.date));
  },
);
export const clearAllData = Effect.fn("PetTracker.clearAllData")(function* () {
  yield* (yield* weightHistoryTable).clear();
  yield* (yield* petsTable).clear();
});
