import { Table } from "@micthiesen/mitools/table";

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

let petsTable: Table<PetRow> | undefined;
let weightHistoryTable: Table<WeightHistoryRow> | undefined;

function getPetsTable(): Table<PetRow> {
  petsTable ??= new Table<PetRow>({
    name: "pets",
    columns: {
      pet_id: { type: "TEXT", primaryKey: true },
      name: { type: "TEXT", notNull: true },
      current_weight: { type: "REAL", notNull: true },
      updated_at: { type: "TEXT", notNull: true },
    },
  });
  return petsTable;
}

function getWeightHistoryTable(): Table<WeightHistoryRow> {
  weightHistoryTable ??= new Table<WeightHistoryRow>({
    name: "pet_weight_history",
    columns: {
      pet_id: { type: "TEXT", notNull: true, primaryKey: true },
      timestamp: { type: "TEXT", notNull: true, primaryKey: true },
      weight: { type: "REAL", notNull: true },
    },
    indexes: [{ columns: ["pet_id", "timestamp"] }],
  });
  return weightHistoryTable;
}

export function upsertPet(pet: PetRow): void {
  getPetsTable().upsert(pet);
}

export function insertWeightReading(reading: WeightHistoryRow): boolean {
  return getWeightHistoryTable().insert(reading);
}

export function getAllPets(): PetRow[] {
  return getPetsTable().all();
}

export function getWeightHistory(petId: string): WeightHistoryRow[] {
  return getWeightHistoryTable().query("pet_id = ? ORDER BY timestamp ASC", [petId]);
}

export function getAllPetsWithHistory(): Array<
  PetRow & { weightHistory: WeightHistoryRow[] }
> {
  return getAllPets().map((pet) => ({
    ...pet,
    weightHistory: getWeightHistory(pet.pet_id),
  }));
}

export function clearAllData(): void {
  getWeightHistoryTable().clear();
  getPetsTable().clear();
}
