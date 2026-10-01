import { describe, expect, it } from "@effect/vitest";
import { Logger } from "@micthiesen/mitools/logging";
import { Effect, Layer } from "effect";
import { Docstore } from "@micthiesen/mitools/docstore";
import { Sqlite } from "@micthiesen/mitools/sqlite";
import { vi } from "vitest";
import { ImapTransport } from "./transport.js";

const sentPath = "Courrier envoyé";
const messageId = "<sent-reply@example.test>";
const receivedAt = new Date("2026-09-30T23:59:10Z");
const textBody = "Hi,\n\nPlease book the follow-up.\n\nThanks,\nMichael";
const source = Buffer.from(
  [
    "From: Michael <me@example.test>",
    "To: clinic@example.test",
    "Cc: copy@example.test",
    `Message-ID: ${messageId}`,
    "In-Reply-To: <parent@example.test>",
    "References: <root@example.test> <parent@example.test>",
    "Subject: Re: Follow-up",
    "Date: Wed, 30 Sep 2026 23:59:10 +0000",
    "Content-Type: text/plain; charset=utf-8",
    "",
    textBody.replace(/\n/g, "\r\n"),
  ].join("\r\n"),
);

function fixture() {
  const mailbox = { path: "INBOX", uidValidity: 42 };
  const release = vi.fn();
  const client = {
    usable: true,
    mailbox,
    list: vi.fn(async () => [
      { path: "INBOX", specialUse: "\\Inbox" },
      { path: "Archive", specialUse: "\\Archive" },
      { path: sentPath, specialUse: "\\Sent" },
    ]),
    getMailboxLock: vi.fn(async (path: string) => {
      mailbox.path = path;
      return { release };
    }),
    mailboxOpen: vi.fn(async (path: string) => {
      mailbox.path = path;
    }),
    search: vi.fn(async () => (mailbox.path === sentPath ? [7] : [])),
    fetch: vi.fn(async function* (uids: number[]) {
      if (mailbox.path !== sentPath) return;
      for (const uid of uids) yield { uid, source, internalDate: receivedAt };
    }),
    fetchOne: vi.fn(async (uid: string) =>
      mailbox.path === sentPath && Number(uid) === 7
        ? { uid: 7, source, internalDate: receivedAt, envelope: { messageId } }
        : false,
    ),
  };
  const transport = new ImapTransport(
    { user: "me@example.test", pass: "fake" },
    Logger.named("TransportSentSpec"),
  );
  Object.assign(transport, { client });
  return { transport, client, release };
}

describe("Sent mailbox transport integration", () => {
  it.effect(
    "searches the designated localized Sent mailbox and directly reads the complete reply",
    () =>
      Effect.gen(function* () {
        const { transport, client, release } = fixture();
        const found = yield* transport.searchEmailsEffect({
          folder: "sent",
          limit: 5,
          fresh: true,
        });
        expect(found).toHaveLength(1);
        expect(found[0]).toMatchObject({
          id: messageId,
          messageId,
          from: "me@example.test",
          to: ["clinic@example.test"],
          cc: ["copy@example.test"],
          subject: "Re: Follow-up",
          textBody,
          inReplyTo: "<parent@example.test>",
          references: ["<root@example.test>", "<parent@example.test>"],
          receivedAt: receivedAt.toISOString(),
        });
        expect(client.getMailboxLock).toHaveBeenCalledExactlyOnceWith(sentPath, {
          readOnly: true,
        });
        expect(client.mailbox.path).toBe("INBOX");
        expect(client.mailboxOpen).toHaveBeenCalledExactlyOnceWith("INBOX", {
          readOnly: true,
        });

        const direct = yield* transport.fetchEmailByIdEffect(found[0].id, {
          fresh: true,
        });
        expect(direct).toEqual(found[0]);
        expect(client.list).toHaveBeenCalledTimes(2);
        expect(client.getMailboxLock.mock.calls.map(([path]) => path)).toEqual([
          sentPath,
          "INBOX",
          "Archive",
          sentPath,
        ]);
        expect(client.fetchOne).toHaveBeenCalledWith(
          "7",
          { source: true, internalDate: true },
          { uid: true },
        );
        expect(client.mailbox.path).toBe("INBOX");
        expect(client.mailboxOpen).toHaveBeenCalledTimes(2);
        expect(release).toHaveBeenCalledTimes(4);
      }).pipe(
        Effect.provide(Logger.layer()),
        Effect.provide(
          Docstore.layer.pipe(Layer.provide(Sqlite.layer({ path: ":memory:" }))),
        ),
      ),
  );

  it.effect("continues polling only Inbox and Archive after Sent search", () =>
    Effect.gen(function* () {
      const { transport, client } = fixture();
      yield* transport.searchEmailsEffect({ folder: "sent", limit: 5, fresh: true });
      const pollFolder = vi.fn((_client: unknown, _folder: string) =>
        Effect.succeed({ emails: [], commit: Effect.void }),
      );
      // Isolate cursor persistence while checking the real polling folder selection.
      Object.assign(transport, { pollFolderEffect: pollFolder, autoReadFolders: [] });
      const poll = yield* transport.pollNewEmailsEffect;
      expect(poll.emails).toEqual([]);
      expect(pollFolder.mock.calls.map(([, path]) => path)).toEqual([
        "INBOX",
        "Archive",
      ]);
      expect(client.list).toHaveBeenCalledOnce();
      expect(client.mailbox.path).toBe("INBOX");
    }).pipe(
      Effect.provide(Logger.layer()),
      Effect.provide(
        Docstore.layer.pipe(Layer.provide(Sqlite.layer({ path: ":memory:" }))),
      ),
    ),
  );
});

it.effect("does not use a substring search candidate as the reply parent", () =>
  Effect.gen(function* () {
    const { transport, client } = fixture();
    client.search.mockImplementation(async () =>
      client.mailbox.path === sentPath ? [7, 8] : [],
    );
    client.fetchOne.mockImplementation(async (uid: string) =>
      client.mailbox.path === sentPath
        ? {
            uid: Number(uid),
            source:
              Number(uid) === 8
                ? Buffer.from(
                    source.toString().replace(messageId, "<unrelated@example.test>"),
                  )
                : source,
            internalDate: receivedAt,
            envelope: {
              messageId: Number(uid) === 8 ? "<unrelated@example.test>" : messageId,
            },
          }
        : false,
    );
    expect(
      (yield* transport.fetchEmailByIdEffect(messageId, { fresh: true }))?.messageId,
    ).toBe(messageId);
    expect(client.fetchOne.mock.calls.map(([uid]) => uid)).toEqual(["8", "7"]);
  }).pipe(Effect.provide(Logger.layer())),
);
