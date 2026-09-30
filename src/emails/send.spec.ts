import { Logger } from "@micthiesen/mitools/logging";
import { ManagedRuntime } from "effect";
import { describe, expect, it, vi } from "vitest";
import { sendComposedEmailEffect } from "./send.js";

const runtime = ManagedRuntime.make(Logger.layer());

describe("sendComposedEmailEffect", () => {
  it("sends the stable message ID and returns true only when every recipient is accepted", async () => {
    const sendMail = vi.fn(async () => ({
      accepted: ["to@example.test", "cc@example.test", "bcc@example.test"],
      rejected: [],
      messageId: "<stable@omni-notify>",
    }));
    const sent = await runtime.runPromise(
      sendComposedEmailEffect(
        {
          to: ["to@example.test"],
          cc: ["cc@example.test"],
          bcc: ["bcc@example.test"],
          from: "me@example.test",
          subject: "Test",
          text: "Body",
          messageId: "<stable@omni-notify>",
        },
        { sendMail } as never,
      ),
    );
    expect(sent).toBe(true);
    expect(sendMail).toHaveBeenCalledWith(
      expect.objectContaining({ messageId: "<stable@omni-notify>" }),
    );
  });

  it("returns false when SMTP partially rejects recipients or is unavailable", async () => {
    const partial = await runtime.runPromise(
      sendComposedEmailEffect(
        {
          to: ["to@example.test", "second@example.test"],
          from: "me@example.test",
          subject: "Test",
          text: "Body",
        },
        {
          sendMail: vi.fn(async () => ({
            accepted: ["to@example.test"],
            rejected: ["second@example.test"],
          })),
        } as never,
      ),
    );
    const absent = await runtime.runPromise(
      sendComposedEmailEffect(
        {
          to: ["to@example.test"],
          from: "me@example.test",
          subject: "Test",
          text: "Body",
        },
        null,
      ),
    );
    expect(partial).toBe(false);
    expect(absent).toBe(false);
  });

  it("does not require duplicate To and Cc addresses to be accepted twice", async () => {
    const sendMail = vi.fn(async () => ({
      accepted: ["same@example.test"],
      rejected: [],
    }));
    const sent = await runtime.runPromise(
      sendComposedEmailEffect(
        {
          to: ["same@example.test"],
          cc: ["same@example.test"],
          from: "me@example.test",
          subject: "Test",
          text: "Body",
        },
        { sendMail } as never,
      ),
    );
    expect(sent).toBe(true);
  });
});
