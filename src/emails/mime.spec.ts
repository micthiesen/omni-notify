import { Effect } from "effect";
import { simpleParser } from "mailparser";
import { describe, expect, it } from "vitest";
import { prepareComposedEmailEffect } from "./mime.js";

describe("composed MIME", () => {
  it("keeps Bcc private, preserves date/body/subject and extends the reply chain", async () => {
    const result = await Effect.runPromise(
      prepareComposedEmailEffect({
        from: "me@example.test",
        to: ["to@example.test"],
        cc: ["cc@example.test"],
        bcc: ["hidden@example.test"],
        subject: "Re: Sam — follow-up",
        text: "Hi,\n\nExact body.\nMichael",
        messageId: "<stable@example.test>",
        date: new Date("2026-09-30T23:59:05Z"),
        inReplyTo: "<parent@example.test>",
        references: ["<root@example.test>"],
      }),
    );
    const wire = await simpleParser(Buffer.from(result.wire, "base64"));
    const copy = await simpleParser(Buffer.from(result.content, "base64"));
    expect(wire.bcc).toBeUndefined();
    expect(JSON.stringify(copy.bcc)).toContain("hidden@example.test");
    for (const parsed of [wire, copy]) {
      expect(parsed.messageId).toBe("<stable@example.test>");
      expect(parsed.date?.toISOString()).toBe("2026-09-30T23:59:05.000Z");
      expect(parsed.subject).toBe("Re: Sam — follow-up");
      expect(parsed.inReplyTo).toBe("<parent@example.test>");
      expect(parsed.references).toEqual([
        "<root@example.test>",
        "<parent@example.test>",
      ]);
      expect(parsed.text?.trimEnd()).toBe("Hi,\n\nExact body.\nMichael");
    }
  });
});
