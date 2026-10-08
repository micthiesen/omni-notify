import { createHash } from "node:crypto";
import type { NamedLogger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { simpleParser } from "mailparser";
import { afterAll, describe, expect, it, vi } from "vitest";
import { encodeStableAttachmentId } from "../../email/imap/attachments.js";
import { ImapTransport } from "../../email/imap/transport.js";
import { createMitoolsTestRuntime } from "../../test/mitools.js";
import type { McpToolDefinition } from "../tool.js";

const mocks = vi.hoisted(() => ({ send: vi.fn() }));
vi.mock("../../emails/client.js", () => ({
  getComposeEmailConfiguration: () => ({ from: "michael@thiesen.dev" }),
}));
vi.mock("../../emails/send.js", () => ({
  sendComposedEmailEffect: (...args: unknown[]) => mocks.send(...args),
}));

const { createEmailAttachmentTools } = await import("./email-attachments.js");
const { createEmailComposeTools } = await import("./email-compose.js");
const testRuntime = createMitoolsTestRuntime();
afterAll(() => testRuntime.dispose());

const messageId = "<synthetic-source@example.test>";
const attachmentId = encodeStableAttachmentId(messageId, "2");
const sha256 = (data: Buffer) => createHash("sha256").update(data).digest("hex");

/** A synthetic received email: text plus one PDF part with an awkward filename. */
function sourceMessage(pdf: Buffer) {
  return Buffer.from(
    [
      `Message-ID: ${messageId}`,
      "From: clinic@example.test",
      "Subject: Your synthetic results",
      'Content-Type: multipart/mixed; boundary="outer"',
      "",
      "--outer",
      "Content-Type: text/plain",
      "",
      "Synthetic results attached.",
      "--outer",
      'Content-Type: application/pdf; name="Résumé de test.pdf"',
      "Content-Disposition: attachment; filename*=utf-8''R%C3%A9sum%C3%A9%20de%20test.pdf",
      "Content-Transfer-Encoding: base64",
      "",
      pdf.toString("base64"),
      "--outer--",
      "",
    ].join("\r\n"),
  );
}

/** Real ImapTransport over an in-memory mailbox keyed by UID (higher is newer). */
function mailbox(initial: Record<number, Buffer>) {
  const messages = { ...initial };
  const client = {
    usable: true,
    mailbox: { path: "INBOX" },
    getMailboxLock: vi.fn(async () => ({ release: vi.fn() })),
    search: vi.fn(async () => Object.keys(messages).map(Number)),
    list: vi.fn(async () => []),
    fetchOne: vi.fn(async (uid: string, query: { source?: unknown }) =>
      query.source
        ? { source: messages[Number(uid)] }
        : { size: messages[Number(uid)].length, envelope: { messageId } },
    ),
    mailboxOpen: vi.fn(async () => undefined),
  };
  const logger = { debug: () => Effect.void } as unknown as NamedLogger;
  const transport = new ImapTransport({ user: "unused", pass: "unused" }, logger);
  const createDraftEffect = vi.fn(() =>
    Effect.succeed({ draftId: "<draft@omni-notify>", alreadyExisted: false }),
  );
  Object.assign(transport, { client, createDraftEffect });
  const runtime = { emailControls: { transport } } as never;
  const tool = (name: string) =>
    [...createEmailAttachmentTools(runtime), ...createEmailComposeTools(runtime)].find(
      (item) => item.name === name,
    )!;
  return { messages, client, createDraftEffect, tool };
}

// Results are loosely typed tool output; the assertions check their shape.
const run = (tool: McpToolDefinition, input: Record<string, unknown>) =>
  testRuntime.run(tool.execute(input)) as Promise<Record<string, any>>;

describe("retrieve, review and send a received PDF", () => {
  it("sends exactly the reviewed bytes, filename and MIME type, and retries safely", async () => {
    const pdf = Buffer.from("%PDF-1.7\n% synthetic, not a real document\n%%EOF\n");
    const box = mailbox({ 7: sourceMessage(pdf) });
    mocks.send.mockReset().mockReturnValue(Effect.succeed(true));

    const reviewed = await run(box.tool("email_attachment_get"), {
      messageId,
      attachmentId,
    });
    const download = box.tool("email_attachment_get").formatResult!(reviewed);
    const blob = (download.content[1] as { resource: { blob: string } }).resource.blob;
    expect(Buffer.from(blob, "base64").equals(pdf)).toBe(true);
    expect(reviewed).toMatchObject({
      filename: "Résumé de test.pdf",
      mimeType: "application/pdf",
      size: pdf.length,
      sha256: sha256(pdf),
      attachmentReference: { messageId, attachmentId, sha256: sha256(pdf) },
    });

    const input = {
      idempotencyKey: "e2e-reviewed-send",
      to: "doctor@example.test",
      subject: "Re: Your synthetic results",
      text: "Attached as requested.",
      inReplyTo: messageId,
      attachments: [reviewed.attachmentReference],
    };
    const sent = await run(box.tool("email_send"), input);
    expect(sent).toMatchObject({
      sent: true,
      alreadySent: false,
      attachments: [
        {
          ...reviewed.attachmentReference,
          filename: "Résumé de test.pdf",
          mimeType: "application/pdf",
          size: pdf.length,
        },
      ],
    });
    const [smtp] = mocks.send.mock.calls[0] as [{ raw: Buffer }];
    const wire = await simpleParser(smtp.raw);
    expect(wire.inReplyTo).toBe(messageId);
    expect(wire.attachments).toHaveLength(1);
    expect(wire.attachments[0]).toMatchObject({
      filename: "Résumé de test.pdf",
      contentType: "application/pdf",
      contentDisposition: "attachment",
    });
    expect(wire.attachments[0].content.equals(pdf)).toBe(true);

    const reads = box.client.fetchOne.mock.calls.length;
    expect(await run(box.tool("email_send"), input)).toEqual({
      ...sent,
      alreadySent: true,
    });
    expect(box.client.fetchOne).toHaveBeenCalledTimes(reads);
    expect(mocks.send).toHaveBeenCalledOnce();
  });

  it("fails closed when a newer copy with the same Message-ID has different bytes", async () => {
    const pdf = Buffer.from("%PDF-1.7\n% reviewed synthetic copy\n%%EOF\n");
    const box = mailbox({ 7: sourceMessage(pdf) });
    mocks.send.mockReset().mockReturnValue(Effect.succeed(true));
    const reviewed = await run(box.tool("email_attachment_get"), {
      messageId,
      attachmentId,
    });
    box.messages[9] = sourceMessage(
      Buffer.from("%PDF-1.7\n% substituted copy\n%%EOF\n"),
    );
    const message = {
      to: "doctor@example.test",
      subject: "Results",
      text: "Attached.",
      attachments: [reviewed.attachmentReference],
    };
    for (const [name, idempotencyKey] of [
      ["email_send", "e2e-substituted-send"],
      ["email_draft_create", "e2e-substituted-draft"],
    ]) {
      await expect(run(box.tool(name), { ...message, idempotencyKey })).rejects.toThrow(
        /no longer matches the reviewed sha256.*Nothing was sent or saved/,
      );
    }
    expect(mocks.send).not.toHaveBeenCalled();
    expect(box.createDraftEffect).not.toHaveBeenCalled();
  });
});
