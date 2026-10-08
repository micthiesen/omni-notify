import { Logger, type NamedLogger } from "@micthiesen/mitools/logging";
import { Effect, ManagedRuntime } from "effect";
import { simpleParser } from "mailparser";
import { describe, expect, it, vi } from "vitest";
import {
  attachmentPartId,
  encodeStableAttachmentId,
  MAX_ATTACHMENT_MESSAGE_BYTES,
  safeAttachmentFilename,
} from "./attachments.js";
import { mapParsedMessage } from "./mapMessage.js";
import { ImapTransport } from "./transport.js";

const runtime = ManagedRuntime.make(Logger.layer());
const messageId = "<attachment@example.test>";
const source = Buffer.from(
  [
    `Message-ID: ${messageId}`,
    'Content-Type: multipart/mixed; boundary="outer"',
    "",
    "--outer",
    "Content-Type: text/plain",
    "",
    "body",
    "--outer",
    'Content-Type: multipart/related; boundary="inner"',
    "",
    "--inner",
    "Content-Type: text/html",
    "",
    '<img src="cid:pic">',
    "--inner",
    "Content-Type: image/png",
    'Content-Disposition: inline; filename="pic.png"',
    "Content-ID: <pic>",
    "Content-Transfer-Encoding: base64",
    "",
    "aW1hZ2U=",
    "--inner--",
    "--outer",
    'Content-Type: application/pdf; name="agreement.pdf"',
    'Content-Disposition: attachment; filename="agreement.pdf"',
    "Content-Transfer-Encoding: base64",
    "",
    Buffer.from("%PDF-1.4\nprivate").toString("base64"),
    "--outer--",
    "",
  ].join("\r\n"),
);

function setup(size = source.length, envelopeId = messageId, actualSource = source) {
  const mailbox = { path: "INBOX" };
  const client = {
    usable: true,
    mailbox,
    getMailboxLock: vi.fn(async (folder: string) => {
      mailbox.path = folder;
      return { release: vi.fn() };
    }),
    search: vi.fn(async () => [7]),
    list: vi.fn(async () => []),
    fetchOne: vi.fn(
      async (
        _uid: string,
        query: { source?: boolean | { start: number; maxLength: number } },
      ) =>
        query.source
          ? { source: actualSource }
          : { size, envelope: { messageId: envelopeId } },
    ),
    mailboxOpen: vi.fn(async (folder: string) => {
      mailbox.path = folder;
    }),
  };
  const logger = { debug: () => Effect.void } as unknown as NamedLogger;
  const transport = new ImapTransport({ user: "unused", pass: "unused" }, logger);
  Object.assign(transport, { client });
  return { transport, client };
}

describe("stable read-only attachments", () => {
  it("uses root MIME part 1 and preserves declared type instead of filename inference", async () => {
    const root = Buffer.from(
      [
        `Message-ID: ${messageId}`,
        "Content-Type: application/octet-stream",
        'Content-Disposition: attachment; filename="disguised.pdf"',
        "Content-Transfer-Encoding: base64",
        "",
        Buffer.from("%PDF-1.4").toString("base64"),
        "",
      ].join("\r\n"),
    );
    const parsed = await simpleParser(root);
    expect(parsed.attachments[0].contentType).toBe("application/pdf");
    expect(attachmentPartId(parsed.attachments[0])).toBe("1");
    const { transport } = setup(root.length, messageId, root);
    const result = await runtime.runPromise(
      transport.fetchAttachmentByIdEffect(
        messageId,
        encodeStableAttachmentId(messageId, "1"),
      ),
    );
    expect(result?.mimeType).toBe("application/octet-stream");
  });

  it("retains nested inline MIME identity and distinct stable handles across moves", async () => {
    const parsed = await simpleParser(source);
    expect(parsed.attachments.map(attachmentPartId)).toEqual(["2.2", "3"]);
    const first = mapParsedMessage(
      parsed,
      { folder: "INBOX", uidValidity: "1", uid: 7 },
      undefined,
    );
    const moved = mapParsedMessage(
      parsed,
      { folder: "Archive", uidValidity: "2", uid: 9 },
      undefined,
    );
    expect(first.attachments.map((a) => a.attachmentId)).toEqual(
      moved.attachments.map((a) => a.attachmentId),
    );
    expect(first.attachments[0]).toMatchObject({
      partId: "2.2",
      disposition: "inline",
      contentId: "<pic>",
    });
    expect(new Set(first.attachments.map((a) => a.attachmentId)).size).toBe(2);
  });

  it("downloads only the selected MIME part with read-only locks", async () => {
    const { transport, client } = setup();
    const result = await runtime.runPromise(
      transport.fetchAttachmentByIdEffect(
        messageId,
        encodeStableAttachmentId(messageId, "3"),
      ),
    );
    expect(result).toMatchObject({
      name: "agreement.pdf",
      mimeType: "application/pdf",
      data: Buffer.from("%PDF-1.4\nprivate"),
    });
    expect(client.getMailboxLock).toHaveBeenCalledWith("INBOX", { readOnly: true });
    expect(client.fetchOne).toHaveBeenCalledWith(
      "7",
      { source: { start: 0, maxLength: MAX_ATTACHMENT_MESSAGE_BYTES + 1 } },
      { uid: true },
    );
  });

  it("rejects oversized messages before fetching source", async () => {
    const { transport, client } = setup(MAX_ATTACHMENT_MESSAGE_BYTES + 1);
    await expect(
      runtime.runPromise(
        transport.fetchAttachmentByIdEffect(
          messageId,
          encodeStableAttachmentId(messageId, "3"),
        ),
      ),
    ).rejects.toThrow("byte limit");
    expect(client.fetchOne).toHaveBeenCalledTimes(1);
  });

  it("checks exact envelope and parsed identity and returns undefined for missing parts", async () => {
    const wrong = setup(source.length, "<other@example.test>");
    expect(
      await runtime.runPromise(
        wrong.transport.fetchAttachmentByIdEffect(
          messageId,
          encodeStableAttachmentId(messageId, "3"),
        ),
      ),
    ).toBeUndefined();
    expect(wrong.client.fetchOne.mock.calls.every(([, q]) => !q.source)).toBe(true);
    const changed = setup(
      source.length,
      messageId,
      Buffer.from(source.toString().replace(messageId, "<other@example.test>")),
    );
    expect(
      await runtime.runPromise(
        changed.transport.fetchAttachmentByIdEffect(
          messageId,
          encodeStableAttachmentId(messageId, "3"),
        ),
      ),
    ).toBeUndefined();
    const absent = setup();
    expect(
      await runtime.runPromise(
        absent.transport.fetchAttachmentByIdEffect(
          messageId,
          encodeStableAttachmentId(messageId, "99"),
        ),
      ),
    ).toBeUndefined();
  });

  it("enforces decoded byte limits and validates inputs before IMAP", async () => {
    const { transport, client } = setup();
    await expect(
      runtime.runPromise(
        transport.fetchAttachmentByIdEffect(
          messageId,
          encodeStableAttachmentId(messageId, "3"),
          { maxBytes: 1 },
        ),
      ),
    ).rejects.toThrow("byte limit");
    client.fetchOne.mockClear();
    await expect(
      runtime.runPromise(transport.fetchAttachmentByIdEffect("bad\r\nidentity", "bad")),
    ).rejects.toThrow("Invalid attachment");
    expect(client.fetchOne).not.toHaveBeenCalled();
  });

  it("sanitizes path, controls and bidi filenames without interpreting sender paths", () => {
    expect(safeAttachmentFilename("../../folder\\offer\u202e.pdf\r\n")).toBe(
      "offer.pdf",
    );
    expect(safeAttachmentFilename("..\u0000")).toBe("attachment");
    expect(safeAttachmentFilename("\ufeffscan\u200b\u0085\u061c.pdf")).toBe("scan.pdf");
    expect(safeAttachmentFilename("a".repeat(300))).toHaveLength(180);
  });
});
