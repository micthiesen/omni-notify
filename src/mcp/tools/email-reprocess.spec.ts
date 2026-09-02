import { describe, expect, it, vi } from "vitest";
import { Effect, Exit } from "effect";
import type { FetchedEmail } from "../../email/types.js";
import { handleEmailThenClearRetryEffect } from "./email-reprocess.js";

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
    const handleEmailsEffect = vi.fn(() => Effect.void);

    await Effect.runPromise(
      handleEmailThenClearRetryEffect({ handleEmailsEffect }, email, () =>
        Effect.sync(clearRetry),
      ),
    );

    expect(handleEmailsEffect).toHaveBeenCalledWith([email]);
    expect(clearRetry).toHaveBeenCalledOnce();
    expect(handleEmailsEffect.mock.invocationCallOrder[0]).toBeLessThan(
      clearRetry.mock.invocationCallOrder[0] ?? 0,
    );
  });

  it("retains the scheduled retry when processing fails", async () => {
    const clearRetry = vi.fn();
    const handleEmailsEffect = vi.fn(() => Effect.fail(new Error("pipeline failed")));

    const exit = await Effect.runPromiseExit(
      handleEmailThenClearRetryEffect({ handleEmailsEffect }, email, () =>
        Effect.sync(clearRetry),
      ),
    );
    expect(Exit.isFailure(exit)).toBe(true);
    expect(clearRetry).not.toHaveBeenCalled();
  });
});
