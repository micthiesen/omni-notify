import { simpleParser } from "mailparser";
import { describe, expect, it } from "vitest";
import { mapParsedMessage } from "./mapMessage.js";

describe("IMAP message mapping", () => {
  it("retains reply recipients and threading headers without treating a fallback ID as a Message-ID", async () => {
    const parsed = await simpleParser(
      [
        "From: Sender <sender@example.test>",
        "To: Reader <reader@example.test>, other@example.test",
        "Cc: copy@example.test",
        "Reply-To: replies@example.test",
        "Message-ID: <message@example.test>",
        "References: <root@example.test> <parent@example.test>",
        "Subject: A message",
        "List-Unsubscribe: <https://example.test/unsubscribe?token=private-test>",
        "List-Unsubscribe-Post: List-Unsubscribe=One-Click",
        "X-Private: must-not-leak",
        "",
        "Body",
      ].join("\r\n"),
    );
    const email = mapParsedMessage(
      parsed,
      { folder: "INBOX", uidValidity: "1", uid: 2 },
      new Date("2026-09-30T12:00:00Z"),
    );
    expect(email).toMatchObject({
      id: "<message@example.test>",
      messageId: "<message@example.test>",
      to: ["reader@example.test", "other@example.test"],
      cc: ["copy@example.test"],
      replyTo: ["replies@example.test"],
      references: ["<root@example.test>", "<parent@example.test>"],
      linkMetadata: {
        listUnsubscribe: {
          urls: ["https://example.test/unsubscribe?token=private-test"],
          post: "List-Unsubscribe=One-Click",
          present: true,
          truncated: false,
        },
      },
    });
    expect(JSON.stringify(email)).not.toContain("must-not-leak");
    const fallback = mapParsedMessage(
      await simpleParser("Subject: No ID\r\n\r\nBody"),
      { folder: "INBOX", uidValidity: "1", uid: 3 },
      undefined,
    );
    expect(fallback.id).toBe("imap|INBOX|1|3");
    expect(fallback.messageId).toBeUndefined();
  });
});
