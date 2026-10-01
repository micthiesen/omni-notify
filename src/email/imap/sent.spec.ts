import { Effect } from "effect";
import { describe, expect, it, vi } from "vitest";
import {
  appendSentCopyEffect,
  findSentCopyEffect,
  type SentCopyClient,
} from "./sent.js";

const messageId = "<Original@example.test>";
const internalDate = new Date("2026-09-30T23:59:10Z");
const content = Buffer.from(
  [
    "From: Michael <me@example.test>",
    "To: clinic@example.test",
    "Cc: copy@example.test",
    "Bcc: private@example.test",
    "Date: Wed, 30 Sep 2026 23:59:10 +0000",
    `Message-ID: ${messageId}`,
    "In-Reply-To: <parent@example.test>",
    "References: <root@example.test> <parent@example.test>",
    "Subject: Re: Follow-up",
    "Content-Type: text/plain; charset=utf-8",
    "",
    "Hi,",
    "",
    "Please book the follow-up.",
  ].join("\r\n"),
);
const input = { messageId, content, internalDate };

function fixture() {
  const rows = new Map<number, { envelope: { messageId: string }; source: Buffer }>();
  const release = vi.fn();
  const client = {
    list: vi.fn(async () => [{ path: "Sent Messages", specialUse: "\\Sent" }]),
    getMailboxLock: vi.fn(async () => ({ release })),
    search: vi.fn(async () => [...rows.keys()]),
    fetchOne: vi.fn(async (uid: number) => rows.get(uid) ?? false),
    append: vi.fn(async (_path: string, bytes: Buffer) => {
      rows.set(1, { envelope: { messageId }, source: bytes });
      return { uid: 1 };
    }),
  } satisfies SentCopyClient;
  return { client, rows, release };
}

const result = <A, E>(effect: Effect.Effect<A, E>) =>
  Effect.runPromise(Effect.result(effect));

describe("Sent copy reconciliation", () => {
  it("discovers Sent, appends the unchanged MIME with original INTERNALDATE, and verifies it", async () => {
    const { client, release } = fixture();
    const saved = await Effect.runPromise(appendSentCopyEffect(client, input));
    expect(saved).toEqual({
      messageId,
      mailbox: "Sent Messages",
      alreadyExisted: false,
    });
    expect(client.append).toHaveBeenCalledExactlyOnceWith(
      "Sent Messages",
      content,
      ["\\Seen"],
      internalDate,
    );
    expect(client.fetchOne).toHaveBeenCalledWith(
      1,
      { envelope: true, source: true },
      { uid: true },
    );
    expect(release).toHaveBeenCalledOnce();
  });

  it("returns an exact preexisting copy without appending", async () => {
    const { client, rows } = fixture();
    rows.set(4, { envelope: { messageId }, source: content });
    const saved = await Effect.runPromise(appendSentCopyEffect(client, input));
    expect(saved.alreadyExisted).toBe(true);
    expect(client.append).not.toHaveBeenCalled();
  });

  it("does a read-only lookup and ignores substring or case-insensitive matches", async () => {
    const { client, rows, release } = fixture();
    rows.set(2, { envelope: { messageId: messageId.toLowerCase() }, source: content });
    rows.set(3, { envelope: { messageId: `${messageId} other` }, source: content });
    expect(
      await Effect.runPromise(findSentCopyEffect(client, messageId)),
    ).toBeUndefined();
    rows.set(4, { envelope: { messageId }, source: content });
    expect(await Effect.runPromise(findSentCopyEffect(client, messageId))).toEqual({
      messageId,
      mailbox: "Sent Messages",
      alreadyExisted: true,
    });
    expect(client.getMailboxLock).toHaveBeenCalledWith("Sent Messages", {
      readOnly: true,
    });
    expect(client.append).not.toHaveBeenCalled();
    expect(release).toHaveBeenCalledTimes(2);
  });

  it.each([
    ["body", "Please book the follow-up.", "Different body"],
    ["from", "me@example.test", "someone@example.test"],
    ["sender name", "From: Michael", "From: Someone"],
    ["to", "clinic@example.test", "different@example.test"],
    ["cc", "copy@example.test", "othercopy@example.test"],
    ["bcc", "private@example.test", "otherprivate@example.test"],
    ["date", "23:59:10", "23:59:11"],
    ["subject", "Re: Follow-up", "Changed subject"],
    [
      "reply",
      "In-Reply-To: <parent@example.test>",
      "In-Reply-To: <other@example.test>",
    ],
    ["references", "<root@example.test>", "<otherroot@example.test>"],
  ])(
    "rejects an existing Message-ID with changed %s",
    async (_kind, original, changed) => {
      const { client, rows, release } = fixture();
      rows.set(1, {
        envelope: { messageId },
        source: Buffer.from(content.toString().replace(original, changed)),
      });
      const outcome = await result(appendSentCopyEffect(client, input));
      expect(outcome._tag).toBe("Failure");
      expect(client.append).not.toHaveBeenCalled();
      expect(release).toHaveBeenCalledOnce();
    },
  );

  it("rejects corruption returned after APPEND and releases the mailbox lock", async () => {
    const { client, rows, release } = fixture();
    client.append.mockImplementation(async () => {
      rows.set(1, {
        envelope: { messageId },
        source: Buffer.from(content.toString().replace("Follow-up", "Wrong")),
      });
      return { uid: 1 };
    });
    expect((await result(appendSentCopyEffect(client, input)))._tag).toBe("Failure");
    expect(client.append).toHaveBeenCalledOnce();
    expect(release).toHaveBeenCalledOnce();
  });

  it("accepts semantically identical MIME with different line endings", async () => {
    const { client, rows } = fixture();
    rows.set(1, {
      envelope: { messageId },
      source: Buffer.from(content.toString().replace(/\r\n/g, "\n")),
    });
    expect(
      (await Effect.runPromise(appendSentCopyEffect(client, input))).alreadyExisted,
    ).toBe(true);
  });

  it.each([
    { folders: [] },
    {
      folders: [
        { path: "One", specialUse: "\\Sent" },
        { path: "Two", specialUse: "\\Sent" },
      ],
    },
  ])(
    "refuses missing or ambiguous server-designated Sent folders",
    async ({ folders }) => {
      const { client } = fixture();
      client.list.mockResolvedValue(folders);
      expect((await result(findSentCopyEffect(client, messageId)))._tag).toBe(
        "Failure",
      );
      expect(client.getMailboxLock).not.toHaveBeenCalled();
      expect(client.append).not.toHaveBeenCalled();
    },
  );

  it("refuses candidate overflow rather than risking a duplicate append", async () => {
    const { client, release } = fixture();
    client.search.mockResolvedValue(Array.from({ length: 51 }, (_, i) => i + 1));
    expect((await result(appendSentCopyEffect(client, input)))._tag).toBe("Failure");
    expect(client.fetchOne).not.toHaveBeenCalled();
    expect(client.append).not.toHaveBeenCalled();
    expect(release).toHaveBeenCalledOnce();
  });

  it("reconciles an uncertain APPEND without issuing another write", async () => {
    const { client, rows, release } = fixture();
    expect(
      (await result(appendSentCopyEffect(client, input, { allowAppend: false })))._tag,
    ).toBe("Failure");
    expect(client.append).not.toHaveBeenCalled();
    rows.set(1, { envelope: { messageId }, source: content });
    expect(
      (
        await Effect.runPromise(
          appendSentCopyEffect(client, input, { allowAppend: false }),
        )
      ).alreadyExisted,
    ).toBe(true);
    expect(client.getMailboxLock).toHaveBeenCalledWith("Sent Messages", {
      readOnly: true,
    });
    expect(client.append).not.toHaveBeenCalled();
    expect(release).toHaveBeenCalledTimes(2);
  });

  it.each(["throw", "false", "invisible"])(
    "never repeats APPEND after a %s outcome",
    async (kind) => {
      const { client, release } = fixture();
      client.append.mockImplementation(async () => {
        if (kind === "throw") throw new Error("connection lost");
        return (kind === "false" ? false : { uid: 1 }) as never;
      });
      expect((await result(appendSentCopyEffect(client, input)))._tag).toBe("Failure");
      expect(client.append).toHaveBeenCalledOnce();
      expect(release).toHaveBeenCalledOnce();
    },
  );

  it("rejects missing authoritative headers or an invalid original date before mailbox access", async () => {
    const { client } = fixture();
    for (const invalid of [
      { ...input, messageId: "<different@example.test>" },
      { ...input, internalDate: new Date("invalid") },
      { ...input, content: Buffer.from(`Message-ID: ${messageId}\r\n\r\nBody`) },
    ]) {
      expect((await result(appendSentCopyEffect(client, invalid)))._tag).toBe(
        "Failure",
      );
    }
    expect(client.list).not.toHaveBeenCalled();
    expect(client.append).not.toHaveBeenCalled();
  });
});

it("claims APPEND only after lookup, and never appends without the durable permit", async () => {
  const { client } = fixture();
  const beforeAppend = Effect.succeed(false);
  const result = await Effect.runPromise(
    Effect.result(appendSentCopyEffect(client, input, { beforeAppend })),
  );
  expect(result._tag).toBe("Failure");
  expect(client.append).not.toHaveBeenCalled();
});
