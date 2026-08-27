import { describe, expect, it, vi } from "vitest";
import type { FetchedEmail } from "../../email/types.js";
import { handleEmailThenClearRetry } from "./email-reprocess.js";

const email: FetchedEmail = {
  id: "message-1",
  subject: "Test",
  from: "sender@example.test",
  receivedAt: "2026-08-26T12:00:00.000Z",
  textBody: "Body",
  links: [],
  attachments: [],
};

describe("manual email reprocessing", () => {
  it("clears the scheduled retry only after successful processing", async () => {
    const clearRetry = vi.fn();
    const handleEmails = vi.fn(async () => undefined);

    await handleEmailThenClearRetry({ handleEmails }, email, clearRetry);

    expect(handleEmails).toHaveBeenCalledWith([email]);
    expect(clearRetry).toHaveBeenCalledOnce();
    expect(handleEmails.mock.invocationCallOrder[0]).toBeLessThan(
      clearRetry.mock.invocationCallOrder[0] ?? 0,
    );
  });

  it("retains the scheduled retry when processing fails", async () => {
    const clearRetry = vi.fn();
    const handleEmails = vi.fn(async () => {
      throw new Error("pipeline failed");
    });

    await expect(
      handleEmailThenClearRetry({ handleEmails }, email, clearRetry),
    ).rejects.toThrow("pipeline failed");
    expect(clearRetry).not.toHaveBeenCalled();
  });
});
