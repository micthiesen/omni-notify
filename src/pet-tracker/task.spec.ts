import { describe, expect, it } from "vitest";
import { petDisplayName } from "./task.js";

describe("petDisplayName", () => {
  it("swaps Whisker's reversed Sam and Sandy profiles", () => {
    expect(petDisplayName("PET-b4738d2e-9a37-4d70-b401-a86e56bfd180", "Sam")).toBe(
      "Sandy",
    );
    expect(petDisplayName("PET-697f1644-6b4b-43cb-945b-61426edcbb86", "Sandy")).toBe(
      "Sam",
    );
  });

  it("keeps Whisker's name for other pets", () => {
    expect(petDisplayName("PET-other", "Luna")).toBe("Luna");
  });
});
