import { createHash } from "node:crypto";
import { Effect } from "effect";
import { simpleParser } from "mailparser";
import { describe, expect, it } from "vitest";
import { prepareComposedEmailEffect } from "./mime.js";

describe("composed MIME", () => {
  it("keeps Bcc private, preserves date/body/subject and extends the reply chain", async () => {
    const result = await Effect.runPromise(
      prepareComposedEmailEffect({
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
    expect(result.from).toBe("michael@thiesen.dev");
    expect(wire.bcc).toBeUndefined();
    expect(JSON.stringify(copy.bcc)).toContain("hidden@example.test");
    for (const parsed of [wire, copy]) {
      expect(parsed.from?.value).toEqual([
        { address: "michael@thiesen.dev", name: "" },
      ]);
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

  it("adds verified PDFs as attachment parts to both the wire and private copy", async () => {
    const first = Buffer.from("%PDF-1.7\nfirst synthetic fixture\n%%EOF");
    const second = Buffer.from("%PDF-1.4\nsecond synthetic fixture\n%%EOF");
    const result = await Effect.runPromise(
      prepareComposedEmailEffect({
        to: ["to@example.test"],
        bcc: ["hidden@example.test"],
        subject: "Re: Results",
        text: "Attached.\nMichael",
        messageId: "<attached@example.test>",
        date: new Date("2026-10-08T12:00:00Z"),
        inReplyTo: "<parent@example.test>",
        references: ["<root@example.test>"],
        attachments: [
          {
            filename: 'Résumé "final".pdf',
            contentType: "application/pdf",
            content: first,
          },
          { filename: "scan.pdf", contentType: "application/pdf", content: second },
        ],
      }),
    );
    const wireBytes = Buffer.from(result.wire, "base64");
    expect(wireBytes.toString("latin1")).toMatch(/^Content-Type: multipart\/mixed/m);
    const wire = await simpleParser(wireBytes);
    const copy = await simpleParser(Buffer.from(result.content, "base64"));
    expect(wire.bcc).toBeUndefined();
    expect(JSON.stringify(copy.bcc)).toContain("hidden@example.test");
    for (const parsed of [wire, copy]) {
      expect(parsed.text?.trimEnd()).toBe("Attached.\nMichael");
      expect(parsed.inReplyTo).toBe("<parent@example.test>");
      expect(parsed.references).toEqual([
        "<root@example.test>",
        "<parent@example.test>",
      ]);
      expect(
        parsed.attachments.map((attachment) => ({
          filename: attachment.filename,
          contentType: attachment.contentType,
          disposition: attachment.contentDisposition,
          sha256: createHash("sha256").update(attachment.content).digest("hex"),
        })),
      ).toEqual(
        [
          ['Résumé "final".pdf', first],
          ["scan.pdf", second],
        ].map(([filename, content]) => ({
          filename,
          contentType: "application/pdf",
          disposition: "attachment",
          sha256: createHash("sha256")
            .update(content as Buffer)
            .digest("hex"),
        })),
      );
    }
  });
});
