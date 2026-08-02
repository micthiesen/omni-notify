import { describe, expect, it } from "vitest";
import { planFolderSync } from "./sync.js";

const cursor = { uidValidity: "1000", uidNext: 50 };

describe("planFolderSync", () => {
  it("initializes on first contact with a folder", () => {
    expect(planFolderSync(undefined, { uidValidity: "1000", uidNext: 50 })).toEqual({
      action: "init",
    });
  });

  it("does nothing when uidNext is unchanged", () => {
    expect(planFolderSync(cursor, { uidValidity: "1000", uidNext: 50 })).toEqual({
      action: "none",
    });
  });

  it("fetches from the cursor when new UIDs exist", () => {
    expect(planFolderSync(cursor, { uidValidity: "1000", uidNext: 53 })).toEqual({
      action: "fetch",
      fromUid: 50,
    });
  });

  it("resets when UIDVALIDITY changes", () => {
    expect(planFolderSync(cursor, { uidValidity: "2000", uidNext: 3 })).toEqual({
      action: "reset",
    });
  });

  it("treats a lower uidNext with same validity as nothing new", () => {
    // Shouldn't happen per RFC 3501, but must not trigger a bogus fetch.
    expect(planFolderSync(cursor, { uidValidity: "1000", uidNext: 40 })).toEqual({
      action: "none",
    });
  });
});
