import { describe, expect, it } from "vitest";
import type { ArchiveAction } from "../../email/archive/persistence.js";
import { serializeAction } from "./email-archive.js";

describe("email archive receipts", () => {
  it("returns the exact new Inbox identity after restore", () => {
    const action: ArchiveAction = {
      actionId: "a".repeat(64),
      identity: {
        folder: "INBOX",
        uidValidity: "10",
        uid: 7,
        messageId: "<selected@example.test>",
      },
      status: "restored",
      destination: { folder: "Archive", uidValidity: "20", uid: 12 },
      restoredLocation: { folder: "INBOX", uidValidity: "10", uid: 8 },
      attempts: 1,
      nextAttemptAt: 0,
      createdAt: 0,
      updatedAt: 1,
    };
    expect(serializeAction(action)).toMatchObject({
      source: action.identity,
      destination: action.destination,
      restoredLocation: action.restoredLocation,
    });
  });
});
