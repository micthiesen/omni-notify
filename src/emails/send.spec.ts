import { Logger } from "@micthiesen/mitools/logging";
import { ManagedRuntime } from "effect";
import { describe, expect, it, vi } from "vitest";
import { sendComposedEmailEffect, sendEmailEffect } from "./send.js";

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
          subject: "Test",
          text: "Body",
          messageId: "<stable@omni-notify>",
        },
        { sendMail } as never,
      ),
    );
    expect(sent).toBe(true);
    expect(sendMail).toHaveBeenCalledWith(
      expect.objectContaining({
        from: "michael@thiesen.dev",
        messageId: "<stable@omni-notify>",
      }),
    );
  });

  it("returns false when SMTP partially rejects recipients or is unavailable", async () => {
    const partial = await runtime.runPromise(
      sendComposedEmailEffect(
        {
          to: ["to@example.test", "second@example.test"],
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
          subject: "Test",
          text: "Body",
        },
        { sendMail } as never,
      ),
    );
    expect(sent).toBe(true);
  });
});

it("sends persisted wire bytes with an explicit deduplicated Bcc envelope", async () => {
  const raw = Buffer.from(
    "From: michael@thiesen.dev\r\nMessage-ID: <wire@test>\r\n\r\nBody",
  );
  const sendMail = vi.fn(async () => ({
    accepted: ["to@test", "cc@test", "hidden@test"],
    rejected: [],
  }));
  expect(
    await runtime.runPromise(
      sendComposedEmailEffect(
        {
          to: ["to@test"],
          cc: ["cc@test", "to@test"],
          bcc: ["hidden@test"],
          subject: "Test",
          text: "Body",
          raw,
        },
        { sendMail } as never,
      ),
    ),
  ).toBe(true);
  expect(sendMail).toHaveBeenCalledWith(
    expect.objectContaining({
      raw,
      envelope: {
        from: "michael@thiesen.dev",
        to: ["to@test", "cc@test", "hidden@test"],
      },
    }),
  );
});

it("refuses persisted MIME with a different From before contacting SMTP", async () => {
  const sendMail = vi.fn();
  expect(
    await runtime.runPromise(
      sendComposedEmailEffect(
        {
          to: ["to@example.test"],
          subject: "Test",
          text: "Body",
          raw: Buffer.from("From: micthiesen@icloud.com\r\n\r\nBody"),
        },
        { sendMail } as never,
      ),
    ),
  ).toBe(false);
  expect(sendMail).not.toHaveBeenCalled();
});

it("uses the fixed identity for notification mail", async () => {
  const sendMail = vi.fn(async () => ({}));
  expect(
    await runtime.runPromise(
      sendEmailEffect(
        {
          to: "to@example.test",
          subject: "Logs",
          text: "Body",
          html: "<p>Body</p>",
        },
        { sendMail } as never,
      ),
    ),
  ).toBe(true);
  expect(sendMail).toHaveBeenCalledWith(
    expect.objectContaining({ from: "michael@thiesen.dev" }),
  );
});
