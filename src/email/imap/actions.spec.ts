import { describe, expect, it, vi } from "vitest";
import { Effect } from "effect";
import { simpleParser } from "mailparser";
import {
  createDraftEffect,
  deterministicDraftMessageId,
  type ImapDraftClient,
} from "./actions.js";
import type { EmailDraftInput } from "../types.js";

const input: EmailDraftInput = {
  idempotencyKey: "draft-key",
  to: ["to@example.test"],
  cc: ["cc@example.test"],
  bcc: ["hidden@example.test"],
  subject: "Reply draft",
  text: "A draft body",
  inReplyTo: "<parent@example.test>",
  references: ["<root@example.test>", "<parent@example.test>"],
};

function draftClient(overrides: Partial<ImapDraftClient> = {}) {
  let appended: Buffer | undefined;
  const client: ImapDraftClient = {
    list: vi.fn(async () => [
      { path: "INBOX" },
      { path: "Drafts.localized", specialUse: "\\Drafts" },
    ]),
    getMailboxLock: vi.fn(async () => ({ release: vi.fn() })),
    search: vi.fn(async () => []),
    fetchOne: vi.fn(async () => false as const),
    append: vi.fn(async (_path, content) => {
      appended = content;
      return "1";
    }),
    ...overrides,
  };
  return {
    client,
    get appended() {
      return appended;
    },
  };
}

describe("createDraftEffect", () => {
  it("discovers the special-use Drafts mailbox and serializes Bcc and reply headers", async () => {
    const messageId = deterministicDraftMessageId(input.idempotencyKey);
    const fixture = draftClient({
      search: vi.fn().mockResolvedValueOnce([]).mockResolvedValueOnce([1]),
      fetchOne: vi.fn(async () => ({ envelope: { messageId } })),
    });
    const result = await Effect.runPromise(createDraftEffect(fixture.client, input));
    const parsed = await simpleParser(fixture.appended!);

    expect(result).toEqual({
      draftId: deterministicDraftMessageId(input.idempotencyKey),
      alreadyExisted: false,
    });
    expect(fixture.client.getMailboxLock).toHaveBeenCalledWith("Drafts.localized", {
      readOnly: false,
    });
    expect(parsed.from?.value).toEqual([{ address: "michael@thiesen.dev", name: "" }]);
    expect(JSON.stringify(parsed.to)).toContain("to@example.test");
    expect(JSON.stringify(parsed.cc)).toContain("cc@example.test");
    expect(JSON.stringify(parsed.bcc)).toContain("hidden@example.test");
    expect(parsed.inReplyTo).toBe("<parent@example.test>");
    expect(parsed.references).toEqual(["<root@example.test>", "<parent@example.test>"]);
  });

  it("attaches verified PDF bytes to the draft MIME without changing reply headers", async () => {
    const messageId = deterministicDraftMessageId(input.idempotencyKey);
    const content = Buffer.from("%PDF-1.7\ndraft synthetic fixture\n%%EOF");
    const fixture = draftClient({
      search: vi.fn().mockResolvedValueOnce([]).mockResolvedValueOnce([1]),
      fetchOne: vi.fn(async () => ({ envelope: { messageId } })),
    });
    await Effect.runPromise(
      createDraftEffect(fixture.client, {
        ...input,
        attachments: [
          { filename: "report.pdf", contentType: "application/pdf", content },
        ],
      }),
    );
    const parsed = await simpleParser(fixture.appended!);
    expect(parsed.text?.trimEnd()).toBe("A draft body");
    expect(parsed.inReplyTo).toBe("<parent@example.test>");
    expect(parsed.references).toEqual(["<root@example.test>", "<parent@example.test>"]);
    expect(parsed.attachments).toHaveLength(1);
    expect(parsed.attachments[0]).toMatchObject({
      filename: "report.pdf",
      contentType: "application/pdf",
      contentDisposition: "attachment",
    });
    expect(parsed.attachments[0].content.equals(content)).toBe(true);
  });

  it("releases the Drafts lock after failures", async () => {
    const release = vi.fn();
    const fixture = draftClient({
      getMailboxLock: vi.fn(async () => ({ release })),
      search: vi.fn(async () => {
        throw new Error("search failed");
      }),
    });
    const result = await Effect.runPromise(
      Effect.result(createDraftEffect(fixture.client, input)),
    );
    expect(result._tag).toBe("Failure");
    expect(release).toHaveBeenCalledOnce();
    expect(fixture.client.append).not.toHaveBeenCalled();
  });

  it("does not report success when APPEND returns false or verification fails", async () => {
    const appendRejected = draftClient({ append: vi.fn(async () => false) });
    const rejectedResult = await Effect.runPromise(
      Effect.result(createDraftEffect(appendRejected.client, input)),
    );
    expect(rejectedResult._tag).toBe("Failure");

    const invisible = draftClient({
      search: vi.fn().mockResolvedValueOnce([]).mockResolvedValueOnce([]),
      append: vi.fn(async () => "1"),
    });
    const invisibleResult = await Effect.runPromise(
      Effect.result(createDraftEffect(invisible.client, input)),
    );
    expect(invisibleResult._tag).toBe("Failure");
  });

  it("reconciles an existing draft and disallows append when reconciliation finds nothing", async () => {
    const messageId = deterministicDraftMessageId(input.idempotencyKey);
    const existing = draftClient({
      search: vi.fn(async () => [7]),
      fetchOne: vi.fn(async () => ({
        envelope: { messageId: messageId.toUpperCase() },
      })),
    });
    await expect(
      Effect.runPromise(
        createDraftEffect(existing.client, input, {
          allowAppend: false,
        }),
      ),
    ).resolves.toEqual({ draftId: messageId, alreadyExisted: true });
    expect(existing.client.append).not.toHaveBeenCalled();

    const absent = draftClient();
    const result = await Effect.runPromise(
      Effect.result(
        createDraftEffect(absent.client, input, {
          allowAppend: false,
        }),
      ),
    );
    expect(result._tag).toBe("Failure");
    expect(absent.client.append).not.toHaveBeenCalled();
  });
});
