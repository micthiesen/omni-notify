import { describe, expect, it, vi } from "vitest";
import { runTest } from "../live-check/testRuntime.js";
import { fetchPetsByUser } from "./api.js";

const mocks = vi.hoisted(() => ({ json: vi.fn() }));

vi.mock("got", () => ({
  default: { post: () => ({ json: mocks.json }) },
}));

describe("fetchPetsByUser", () => {
  it("treats a null weight history as no recent readings", async () => {
    mocks.json.mockResolvedValue({
      data: {
        getPetsByUser: [
          {
            petId: "pet-1",
            name: "Sam",
            weight: 12.6,
            lastWeightReading: 12.6,
            weightHistory: null,
          },
        ],
      },
    });

    const pets = await runTest(fetchPetsByUser("token", "user"));

    expect(pets).toEqual([
      {
        petId: "pet-1",
        name: "Sam",
        weight: 12.6,
        lastWeightReading: 12.6,
        weightHistory: [],
      },
    ]);
  });
});
