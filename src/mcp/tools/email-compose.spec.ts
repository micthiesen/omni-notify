import { createHash } from "node:crypto";
import { decodeDoc, Docstore } from "@micthiesen/mitools/docstore";
import { Effect, Option } from "effect";
import { simpleParser } from "mailparser";
import { afterAll, describe, expect, it, vi } from "vitest";
import { createMitoolsTestRuntime } from "../../test/mitools.js";

const mocks = vi.hoisted(() => ({
  send: vi.fn(),
  configuration: { from: "michael@thiesen.dev" },
}));

vi.mock("../../emails/client.js", () => ({
  getComposeEmailConfiguration: () => mocks.configuration,
}));
vi.mock("../../emails/send.js", () => ({
  sendComposedEmailEffect: (...args: unknown[]) => mocks.send(...args),
}));

const { createEmailComposeTools } = await import("./email-compose.js");
const testRuntime = createMitoolsTestRuntime();
const sendTool = createEmailComposeTools({
  logger: testRuntime.logger,
  effectRunner: testRuntime.runner,
  registry: {} as never,
  streamers: [],
  emailControls: { transport: null } as never,
} as never).find((tool) => tool.name === "email_send")!;
const draftTool = createEmailComposeTools({
  logger: testRuntime.logger,
  effectRunner: testRuntime.runner,
  registry: {} as never,
  streamers: [],
  emailControls: { transport: null } as never,
} as never).find((tool) => tool.name === "email_draft_create")!;

const message = (overrides: Record<string, unknown> = {}) => ({
  idempotencyKey: "send-key",
  to: "to@example.test",
  subject: "Test",
  text: "Body",
  ...overrides,
});
const run = (
  tool: typeof sendTool | typeof draftTool,
  input: Record<string, unknown>,
) => testRuntime.run(tool.execute(input));

const receiptKey = (kind: "draft" | "send", key: string) =>
  `email-compose:${kind}:${createHash("sha256").update(key).digest("hex")}`;
const readReceipt = (kind: "draft" | "send", key: string) =>
  testRuntime.run(
    Effect.gen(function* () {
      const row = yield* (yield* Docstore).getRawRow(receiptKey(kind, key));
      return Option.isSome(row)
        ? decodeDoc<Record<string, Record<string, unknown>>>(row.value.data)
        : undefined;
    }),
  );
const writeReceipt = (key: string, data: Record<string, unknown>) =>
  testRuntime.run(
    Effect.gen(function* () {
      yield* (yield* Docstore).upsertDoc(receiptKey("send", key), data, {
        entity: "email-compose-send",
      });
    }),
  );

afterAll(() => testRuntime.dispose());

describe("email compose MCP idempotency", () => {
  it("rejects caller-selected sender fields on sends, replies and drafts", async () => {
    mocks.send.mockReset();
    for (const tool of [sendTool, draftTool]) {
      for (const field of ["from", "sender"]) {
        expect(
          tool.inputSchema.safeParse(message({ [field]: "micthiesen@icloud.com" }))
            .success,
        ).toBe(false);
        await expect(
          run(
            tool,
            message({
              [field]: "micthiesen@icloud.com",
              inReplyTo: "<parent@example.test>",
            }),
          ),
        ).rejects.toThrow();
      }
    }
    expect(mocks.send).not.toHaveBeenCalled();
  });

  it("returns the stored success on retry without sending twice", async () => {
    mocks.send.mockReset().mockReturnValue(Effect.succeed(true));
    const first = await run(sendTool, message());
    const retry = await run(sendTool, message());
    expect(first).toMatchObject({ sent: true, alreadySent: false });
    expect(retry).toEqual({ ...first, alreadySent: true });
    expect(mocks.send).toHaveBeenCalledOnce();
  });

  it("rejects changed content for a previously reserved key", async () => {
    mocks.send.mockReset().mockReturnValue(Effect.succeed(true));
    await run(sendTool, message({ idempotencyKey: "conflict-key" }));
    await expect(
      run(sendTool, message({ idempotencyKey: "conflict-key", text: "Changed" })),
    ).rejects.toThrow(/different send content/);
    expect(mocks.send).toHaveBeenCalledOnce();
  });

  it("keeps uncertain failures pending and never resends automatically", async () => {
    mocks.send.mockReset().mockReturnValue(Effect.interrupt);
    await expect(
      run(sendTool, message({ idempotencyKey: "uncertain-key" })),
    ).rejects.toThrow();
    await expect(
      run(sendTool, message({ idempotencyKey: "uncertain-key" })),
    ).rejects.toThrow(/uncertain outcome/);
    expect(mocks.send).toHaveBeenCalledOnce();
  });

  it("reconciles a pending draft with allowAppend false", async () => {
    const createDraftEffect = vi
      .fn()
      .mockReturnValueOnce(Effect.fail(new Error("uncertain append")))
      .mockReturnValueOnce(
        Effect.succeed({ draftId: "<draft@omni-notify>", alreadyExisted: true }),
      );
    const tool = createEmailComposeTools({
      logger: testRuntime.logger,
      effectRunner: testRuntime.runner,
      registry: {} as never,
      streamers: [],
      emailControls: { transport: { createDraftEffect } } as never,
    } as never).find((item) => item.name === "email_draft_create")!;
    const key = "pending-draft";
    await expect(run(tool, message({ idempotencyKey: key }))).rejects.toThrow(
      "uncertain append",
    );
    const result = await run(tool, message({ idempotencyKey: key }));
    expect(result).toMatchObject({
      draftId: "<draft@omni-notify>",
      alreadyExisted: true,
    });
    expect(createDraftEffect).toHaveBeenCalledTimes(2);
    expect(createDraftEffect.mock.calls[1]?.[1]).toMatchObject({ allowAppend: false });
  });

  it("reserves a concurrent key once, so only one send reaches SMTP", async () => {
    let finish!: (sent: boolean) => void;
    mocks.send.mockReset().mockReturnValue(
      Effect.promise(
        () =>
          new Promise<boolean>((resolve) => {
            finish = resolve;
          }),
      ),
    );
    const first = run(sendTool, message({ idempotencyKey: "concurrent-key" }));
    await vi.waitFor(() => expect(mocks.send).toHaveBeenCalledOnce());
    await expect(
      run(sendTool, message({ idempotencyKey: "concurrent-key" })),
    ).rejects.toThrow(/uncertain outcome/);
    finish(true);
    await expect(first).resolves.toMatchObject({ sent: true, alreadySent: false });
    expect(mocks.send).toHaveBeenCalledOnce();
  });

  it("replays a legacy receipt under the fingerprint computed before attachments", async () => {
    mocks.send.mockReset();
    // Parsed field order and sender exactly as hashed before attachments existed.
    const legacy = createHash("sha256")
      .update(
        JSON.stringify({
          idempotencyKey: "legacy-key",
          to: ["to@example.test"],
          cc: ["cc@example.test"],
          subject: "Test",
          text: "Body",
          inReplyTo: "<parent@example.test>",
          references: ["<root@example.test>"],
          from: "michael@thiesen.dev",
        }),
      )
      .digest("hex");
    await writeReceipt("legacy-key", {
      fingerprint: legacy,
      status: "succeeded",
      result: { sent: true, messageId: `<${legacy}@omni-notify>` },
      prepared: {
        from: "michael@thiesen.dev",
        date: "2026-10-01T00:00:00.000Z",
        wire: "d2lyZQ==",
        content: "Y29weQ==",
      },
      sentCopy: "verified",
      updatedAt: 1,
    });
    const input = message({
      idempotencyKey: "legacy-key",
      cc: ["cc@example.test"],
      inReplyTo: "<parent@example.test>",
      references: ["<root@example.test>"],
    });
    for (const replay of [input, { ...input, attachments: [] }]) {
      expect(await run(sendTool, replay)).toEqual({
        sent: true,
        messageId: `<${legacy}@omni-notify>`,
        alreadySent: true,
        sentCopy: "verified",
        attachments: [],
      });
    }
    expect(mocks.send).not.toHaveBeenCalled();
  });

  it("keeps do-not-repeat guidance visible when an accepted send cannot be recorded", async () => {
    const key = "unrecordable-key";
    mocks.send.mockReset().mockImplementation(() =>
      Effect.gen(function* () {
        yield* (yield* Docstore).upsertDoc(receiptKey("send", key), {
          fingerprint: "other",
          status: "pending",
          updatedAt: 1,
        });
        return true;
      }),
    );
    await expect(run(sendTool, message({ idempotencyKey: key }))).rejects.toThrow(
      /^Could not persist email send outcome; do not repeat with a new key: /,
    );
  });

  it("completes a draft reconciled after a concurrent caller recorded it", async () => {
    const deferred = () => {
      let resolve!: (value: unknown) => void;
      const promise = new Promise((done) => {
        resolve = done;
      });
      return { promise, resolve };
    };
    const [first, second] = [deferred(), deferred()];
    const createDraftEffect = vi
      .fn()
      .mockReturnValueOnce(Effect.promise(() => first.promise))
      .mockReturnValueOnce(Effect.promise(() => second.promise));
    const tool = createEmailComposeTools({
      emailControls: { transport: { createDraftEffect } },
    } as never).find((item) => item.name === "email_draft_create")!;
    const input = message({ idempotencyKey: "draft-race" });
    const draftId = "<draft-race@omni-notify>";
    const winner = run(tool, input);
    await vi.waitFor(() => expect(createDraftEffect).toHaveBeenCalledOnce());
    const reconciler = run(tool, input);
    await vi.waitFor(() => expect(createDraftEffect).toHaveBeenCalledTimes(2));
    expect(createDraftEffect.mock.calls[1]?.[1]).toMatchObject({ allowAppend: false });
    first.resolve({ draftId, alreadyExisted: false });
    await expect(winner).resolves.toMatchObject({ draftId, alreadyExisted: false });
    second.resolve({ draftId, alreadyExisted: true });
    await expect(reconciler).resolves.toMatchObject({ draftId, alreadyExisted: true });
  });
});

describe("durable Sent recovery", () => {
  const toolsWithCopy = (saveSentCopyEffect: unknown) =>
    createEmailComposeTools({
      emailControls: { transport: { saveSentCopyEffect } },
    } as never);
  it("records SMTP success separately, then only reconciles a failed copy on retry", async () => {
    const copy = vi
      .fn()
      .mockImplementationOnce((_input, options) =>
        options.beforeAppend.pipe(
          Effect.andThen(Effect.fail(new Error("lost APPEND response"))),
        ),
      )
      .mockReturnValue(
        Effect.succeed({
          messageId: "<copy@test>",
          mailbox: "Sent",
          alreadyExisted: true,
        }),
      );
    const tools = toolsWithCopy(copy);
    const send = tools.find((t) => t.name === "email_send")!;
    const status = tools.find((t) => t.name === "email_send_status")!;
    const repair = tools.find((t) => t.name === "email_sent_copy_repair")!;
    mocks.send.mockReset().mockReturnValue(Effect.succeed(true));
    const input = message({ idempotencyKey: "sent-recovery" });
    const first = await run(send, input);
    expect(first).toMatchObject({
      sent: true,
      sentCopy: "uncertain",
      alreadySent: false,
    });
    expect(await run(status, { idempotencyKey: input.idempotencyKey })).toMatchObject({
      smtpAccepted: true,
      sentCopy: "uncertain",
    });
    expect(copy).toHaveBeenCalledOnce();
    expect(copy.mock.calls[0]?.[1]).toMatchObject({ allowAppend: true });
    expect(await run(repair, { idempotencyKey: input.idempotencyKey })).toEqual({
      sentCopy: "verified",
    });
    expect(copy.mock.calls[1]?.[1]).toMatchObject({ allowAppend: false });
    expect(copy.mock.calls[1]?.[0].content).toEqual(copy.mock.calls[0]?.[0].content);
    expect(await run(send, input)).toMatchObject({
      sent: true,
      alreadySent: true,
      sentCopy: "verified",
    });
    expect(mocks.send).toHaveBeenCalledOnce();
    expect(copy).toHaveBeenCalledTimes(2);
  });
  it("never appends after partial SMTP acceptance and reports missing receipts truthfully", async () => {
    const copy = vi.fn();
    const tools = toolsWithCopy(copy);
    mocks.send.mockReset().mockReturnValue(Effect.succeed(false));
    const key = "sent-partial";
    await expect(
      run(
        tools.find((t) => t.name === "email_send")!,
        message({ idempotencyKey: key }),
      ),
    ).rejects.toThrow(/Some recipients may/);
    await expect(
      run(
        tools.find((t) => t.name === "email_sent_copy_repair")!,
        { idempotencyKey: key },
      ),
    ).rejects.toThrow(/not confirmed/);
    expect(copy).not.toHaveBeenCalled();
    expect(
      await run(
        tools.find((t) => t.name === "email_send_status")!,
        { idempotencyKey: "missing" },
      ),
    ).toMatchObject({ found: false, smtpAccepted: false, status: null });
  });
});

it("keeps pre-APPEND outages repairable without retransmitting SMTP", async () => {
  const copy = vi
    .fn()
    .mockReturnValueOnce(Effect.fail(new Error("disconnected")))
    .mockImplementation((_input, options) =>
      options.beforeAppend.pipe(
        Effect.map(() => ({
          messageId: "<copy@test>",
          mailbox: "Sent",
          alreadyExisted: false,
        })),
      ),
    );
  const tools = createEmailComposeTools({
    emailControls: { transport: { saveSentCopyEffect: copy } },
  } as never);
  mocks.send.mockReset().mockReturnValue(Effect.succeed(true));
  const key = "disconnected-copy";
  expect(
    await run(
      tools.find((t) => t.name === "email_send")!,
      message({ idempotencyKey: key }),
    ),
  ).toMatchObject({ sent: true, sentCopy: "pending" });
  expect(
    await run(
      tools.find((t) => t.name === "email_sent_copy_repair")!,
      { idempotencyKey: key },
    ),
  ).toEqual({ sentCopy: "verified" });
  expect(copy.mock.calls[1]?.[1]).toMatchObject({ allowAppend: true });
  expect(mocks.send).toHaveBeenCalledOnce();
});

describe("email attachments by stable reference", () => {
  const MiB = 1024 * 1024;
  const source = "<source@example.test>";
  const idFor = (n: number) => `imap-attachment:${n.toString(16).padStart(64, "0")}`;
  const sha256 = (data: Buffer) => createHash("sha256").update(data).digest("hex");
  // Without reviewed bytes, a well-formed pin that no fixture matches.
  const ref = (n: number, reviewed?: Buffer) => ({
    messageId: source,
    attachmentId: idFor(n),
    sha256: reviewed ? sha256(reviewed) : "0".repeat(64),
  });
  const pdf = (label: string, size?: number) => {
    const bytes = Buffer.from(`%PDF-1.7\n${label} synthetic fixture\n%%EOF`);
    return size === undefined
      ? bytes
      : Buffer.concat([bytes, Buffer.alloc(size - bytes.length)]);
  };
  type Source = { name: string; mimeType: string; data: Buffer } | undefined | Error;
  const metadataFor = (n: number, filename: string, data: Buffer) => ({
    ...ref(n, data),
    filename,
    mimeType: "application/pdf",
    size: data.length,
  });

  function fixture(
    sources: Record<string, Source>,
    extra: Record<string, unknown> = {},
  ) {
    const fetch = vi.fn((_messageId: string, attachmentId: string) => {
      const found = sources[attachmentId];
      return found instanceof Error ? Effect.fail(found) : Effect.succeed(found);
    });
    const tools = createEmailComposeTools({
      emailControls: { transport: { fetchAttachmentByIdEffect: fetch, ...extra } },
    } as never);
    const tool = (name: string) => tools.find((item) => item.name === name)!;
    return {
      fetch,
      send: tool("email_send"),
      status: tool("email_send_status"),
      repair: tool("email_sent_copy_repair"),
      draft: tool("email_draft_create"),
    };
  }

  it("sends several re-read PDFs in the persisted wire MIME with safe names and threading", async () => {
    const lab = pdf("lab");
    const scan = pdf("scan");
    const { fetch, send, status } = fixture({
      [idFor(1)]: {
        name: '../Lab "results".pdf',
        mimeType: "application/pdf",
        data: lab,
      },
      [idFor(2)]: { name: "scan", mimeType: "application/pdf", data: scan },
    });
    mocks.send.mockReset().mockReturnValue(Effect.succeed(true));
    const result = await run(
      send,
      message({
        idempotencyKey: "attach-send",
        subject: "Re: Results",
        bcc: ["hidden@example.test"],
        inReplyTo: "<parent@example.test>",
        references: ["<root@example.test>"],
        attachments: [ref(1, lab), ref(2, scan)],
      }),
    );
    expect(fetch.mock.calls).toEqual([
      [source, idFor(1), { maxBytes: 5 * MiB }],
      [source, idFor(2), { maxBytes: 5 * MiB }],
    ]);
    const expected = [
      metadataFor(1, 'Lab "results".pdf', lab),
      metadataFor(2, "scan.pdf", scan),
    ];
    expect(result).toMatchObject({
      sent: true,
      alreadySent: false,
      attachments: expected,
    });

    const smtp = mocks.send.mock.calls[0]?.[0] as { raw: Buffer; bcc: string[] };
    expect(smtp.bcc).toEqual(["hidden@example.test"]);
    const wire = await simpleParser(smtp.raw);
    expect(wire.messageId).toBe(result.messageId);
    expect(wire.bcc).toBeUndefined();
    expect(wire.subject).toBe("Re: Results");
    expect(wire.text?.trimEnd()).toBe("Body");
    expect(wire.inReplyTo).toBe("<parent@example.test>");
    expect(wire.references).toEqual(["<root@example.test>", "<parent@example.test>"]);
    expect(
      wire.attachments.map((part) => [
        part.filename,
        part.contentType,
        part.contentDisposition,
        sha256(part.content),
      ]),
    ).toEqual(
      expected.map((item) => [
        item.filename,
        "application/pdf",
        "attachment",
        item.sha256,
      ]),
    );
    expect(await run(status, { idempotencyKey: "attach-send" })).toMatchObject({
      smtpAccepted: true,
      attachments: expected,
    });
  });

  it("keeps attachment-free sends compatible and treats an empty list as none", async () => {
    const { fetch, send } = fixture({});
    mocks.send.mockReset().mockReturnValue(Effect.succeed(true));
    const first = await run(send, message({ idempotencyKey: "attach-compat" }));
    const retry = await run(
      send,
      message({ idempotencyKey: "attach-compat", attachments: [] }),
    );
    expect(first).toMatchObject({ alreadySent: false, attachments: [] });
    expect(retry).toEqual({ ...first, alreadySent: true });
    expect(fetch).not.toHaveBeenCalled();
    expect(mocks.send).toHaveBeenCalledOnce();
  });

  it("returns the stored receipt on retry without re-reading sources or resending", async () => {
    const invoice = pdf("a");
    const other = pdf("b");
    const sources: Record<string, Source> = {
      [idFor(3)]: { name: "invoice.pdf", mimeType: "application/pdf", data: invoice },
      [idFor(4)]: { name: "other.pdf", mimeType: "application/pdf", data: other },
    };
    const { fetch, send } = fixture(sources);
    mocks.send.mockReset().mockReturnValue(Effect.succeed(true));
    const input = message({
      idempotencyKey: "attach-retry",
      attachments: [ref(3, invoice)],
    });
    const first = await run(send, input);
    sources[idFor(3)] = undefined;
    expect(await run(send, input)).toEqual({ ...first, alreadySent: true });
    await expect(run(send, { ...input, attachments: [ref(4, other)] })).rejects.toThrow(
      /different send content/,
    );
    expect(fetch).toHaveBeenCalledOnce();
    expect(mocks.send).toHaveBeenCalledOnce();
  });

  it("never re-reads or resends after an uncertain SMTP outcome", async () => {
    const data = pdf("a");
    const { fetch, send } = fixture({
      [idFor(5)]: { name: "a.pdf", mimeType: "application/pdf", data },
    });
    mocks.send.mockReset().mockReturnValue(Effect.interrupt);
    const input = message({
      idempotencyKey: "attach-uncertain",
      attachments: [ref(5, data)],
    });
    await expect(run(send, input)).rejects.toThrow();
    await expect(run(send, input)).rejects.toThrow(/uncertain outcome/);
    expect(fetch).toHaveBeenCalledOnce();
    expect(mocks.send).toHaveBeenCalledOnce();
  });

  it("rejects malformed references and caller-supplied bytes before IMAP or SMTP", async () => {
    const { fetch, send, draft } = fixture({});
    mocks.send.mockReset();
    for (const attachments of [
      [{ messageId: source, attachmentId: "../private.pdf" }],
      [{ messageId: "source@example.test", attachmentId: idFor(1) }],
      [{ messageId: "<a@b>\r\nBcc: x@example.test", attachmentId: idFor(1) }],
      [ref(1), ref(1)],
      [1, 2, 3, 4, 5, 6].map((n) => ref(n)),
      [{ ...ref(1), content: pdf("x").toString("base64") }],
      [{ ...ref(1), path: "/etc/passwd" }],
      [{ ...ref(1), filename: "renamed.pdf" }],
      [{ messageId: source, attachmentId: idFor(1) }],
      [{ ...ref(1), sha256: "A".repeat(64) }],
      [{ ...ref(1), sha256: "abc" }],
      ref(1),
    ]) {
      for (const tool of [send, draft]) {
        await expect(
          run(tool, message({ idempotencyKey: "attach-malformed", attachments })),
        ).rejects.toThrow();
      }
    }
    expect(fetch).not.toHaveBeenCalled();
    expect(mocks.send).not.toHaveBeenCalled();
  });

  it("fails before reserving for missing, stale, non-PDF and oversized sources", async () => {
    const name = "Private Diagnosis.pdf";
    const valid = (data: Buffer) => ({ name, mimeType: "application/pdf", data });
    const cases: Array<[Source[], RegExp, pinned?: boolean]> = [
      [[undefined], /was not found in Inbox, Archive or Sent/],
      [
        [new Error("find attachment message failed: socket closed")],
        /could not be read/,
      ],
      [[{ name, mimeType: "text/html", data: pdf("html") }], /not a PDF/],
      [[valid(Buffer.from("plain text"))], /not a PDF/],
      [[valid(pdf("big", 5 * MiB + 1))], /5 MiB attachment limit/],
      [[1, 2, 3].map((n) => valid(pdf(`${n}`, 4 * MiB))), /10 MiB total limit/, true],
      [[valid(pdf("changed"))], /no longer matches the reviewed sha256/],
    ];
    mocks.send.mockReset().mockReturnValue(Effect.succeed(true));
    for (const [index, [sources, error, pinned]] of cases.entries()) {
      const key = `attach-invalid-${index}`;
      const tools = fixture(
        Object.fromEntries(sources.map((item, n) => [idFor(n + 10), item])),
      );
      const failure = run(
        tools.send,
        message({
          idempotencyKey: key,
          attachments: sources.map((item, n) =>
            // Pin the reviewed bytes only where a later check must be reached.
            ref(
              n + 10,
              pinned && item && !(item instanceof Error) ? item.data : undefined,
            ),
          ),
        }),
      ).catch((cause: unknown) =>
        cause instanceof Error ? cause.message : String(cause),
      );
      const reason = await failure;
      expect(reason).toMatch(error);
      expect(reason).toMatch(/Nothing was sent or saved/);
      expect(reason).not.toMatch(/Private Diagnosis/);
      expect(await run(tools.status, { idempotencyKey: key })).toMatchObject({
        found: false,
      });
    }
    expect(mocks.send).not.toHaveBeenCalled();

    const fixed = pdf("fixed");
    const recovered = fixture({ [idFor(10)]: valid(fixed) });
    await expect(
      run(
        recovered.send,
        message({ idempotencyKey: "attach-invalid-0", attachments: [ref(10, fixed)] }),
      ),
    ).resolves.toMatchObject({ sent: true, alreadySent: false });
  });

  it("refuses attachments when stable retrieval is unavailable", async () => {
    mocks.send.mockReset();
    const tools = createEmailComposeTools({
      emailControls: { transport: null },
    } as never);
    await expect(
      run(
        tools.find((tool) => tool.name === "email_send")!,
        message({ idempotencyKey: "attach-unavailable", attachments: [ref(1)] }),
      ),
    ).rejects.toThrow(/retrieval is unavailable/);
    expect(mocks.send).not.toHaveBeenCalled();
  });

  it("repairs an uncertain Sent copy from persisted attachment MIME without resending", async () => {
    const data = pdf("sent-copy");
    const copy = vi
      .fn()
      .mockImplementationOnce((_input, options) =>
        options.beforeAppend.pipe(
          Effect.andThen(Effect.fail(new Error("lost APPEND response"))),
        ),
      )
      .mockReturnValue(
        Effect.succeed({
          messageId: "<copy@test>",
          mailbox: "Sent",
          alreadyExisted: true,
        }),
      );
    const { fetch, send, repair, status } = fixture(
      { [idFor(6)]: { name: "statement.pdf", mimeType: "application/pdf", data } },
      { saveSentCopyEffect: copy },
    );
    mocks.send.mockReset().mockReturnValue(Effect.succeed(true));
    const input = message({
      idempotencyKey: "attach-sent-copy",
      bcc: ["hidden@example.test"],
      attachments: [ref(6, data)],
    });
    expect(await run(send, input)).toMatchObject({ sent: true, sentCopy: "uncertain" });
    const pending = await readReceipt("send", "attach-sent-copy");
    expect(pending?.prepared).not.toHaveProperty("wire");
    expect(pending?.prepared).toHaveProperty("content");
    expect(await run(repair, { idempotencyKey: "attach-sent-copy" })).toEqual({
      sentCopy: "verified",
    });
    expect(
      Object.keys((await readReceipt("send", "attach-sent-copy"))!.prepared),
    ).toEqual(["from", "date"]);
    expect(await run(status, { idempotencyKey: "attach-sent-copy" })).toMatchObject({
      sentCopy: "verified",
      messageDate: expect.any(String),
      attachments: [metadataFor(6, "statement.pdf", data)],
    });
    expect(copy.mock.calls[1]?.[1]).toMatchObject({ allowAppend: false });
    expect(copy.mock.calls[1]?.[0].content).toEqual(copy.mock.calls[0]?.[0].content);
    const saved = await simpleParser(copy.mock.calls[1]?.[0].content);
    expect(JSON.stringify(saved.bcc)).toContain("hidden@example.test");
    expect(
      saved.attachments.map((part) => [part.filename, sha256(part.content)]),
    ).toEqual([["statement.pdf", sha256(data)]]);
    expect(fetch).toHaveBeenCalledOnce();
    expect(mocks.send).toHaveBeenCalledOnce();
  });

  it("drafts re-read attachments once and reconcile a pending draft from its receipt", async () => {
    const data = pdf("draft");
    const createDraftEffect = vi
      .fn()
      .mockReturnValueOnce(Effect.fail(new Error("uncertain append")))
      .mockReturnValueOnce(
        Effect.succeed({ draftId: "<draft-attach@omni-notify>", alreadyExisted: true }),
      );
    const { fetch, draft } = fixture(
      { [idFor(7)]: { name: "form", mimeType: "application/pdf", data } },
      { createDraftEffect },
    );
    const input = message({
      idempotencyKey: "attach-draft",
      attachments: [ref(7, data)],
    });
    await expect(run(draft, input)).rejects.toThrow("uncertain append");
    expect(createDraftEffect.mock.calls[0]?.[0].attachments).toEqual([
      { filename: "form.pdf", contentType: "application/pdf", content: data },
    ]);
    const reconciled = await run(draft, input);
    expect(reconciled).toEqual({
      draftId: "<draft-attach@omni-notify>",
      alreadyExisted: true,
      attachments: [metadataFor(7, "form.pdf", data)],
    });
    expect(createDraftEffect.mock.calls[1]?.[1]).toMatchObject({ allowAppend: false });
    expect(await run(draft, input)).toEqual(reconciled);
    expect(createDraftEffect).toHaveBeenCalledTimes(2);
    expect(fetch).toHaveBeenCalledOnce();
  });

  it("reserves no draft for a missing source, then returns attached metadata", async () => {
    const data = pdf("draft-ok");
    const createDraftEffect = vi.fn(() =>
      Effect.succeed({ draftId: "<draft-ok@omni-notify>", alreadyExisted: false }),
    );
    const sources: Record<string, Source> = { [idFor(9)]: undefined };
    const { draft } = fixture(sources, { createDraftEffect });
    const input = message({
      idempotencyKey: "attach-draft-ok",
      attachments: [ref(9, data)],
    });
    await expect(run(draft, input)).rejects.toThrow(/was not found/);
    expect(createDraftEffect).not.toHaveBeenCalled();
    expect(await readReceipt("draft", "attach-draft-ok")).toBeUndefined();
    sources[idFor(9)] = { name: "lease.pdf", mimeType: "application/pdf", data };
    expect(await run(draft, input)).toEqual({
      draftId: "<draft-ok@omni-notify>",
      alreadyExisted: false,
      attachments: [metadataFor(9, "lease.pdf", data)],
    });
  });

  it("breaks up encoded-word markers so recipients cannot decode header text", async () => {
    const data = pdf("encoded");
    const { send } = fixture({
      [idFor(11)]: {
        name: "=?utf-8?Q?evil=0D=0ABcc:x@y?=.pdf",
        mimeType: "application/pdf",
        data,
      },
    });
    mocks.send.mockReset().mockReturnValue(Effect.succeed(true));
    await run(
      send,
      message({ idempotencyKey: "attach-encoded", attachments: [ref(11, data)] }),
    );
    const [smtp] = mocks.send.mock.calls[0] as [{ raw: Buffer }];
    const wire = await simpleParser(smtp.raw);
    expect(wire.attachments.map((part) => part.filename)).toEqual([
      "=_utf-8?Q?evil=0D=0ABcc:x@y?=.pdf",
    ]);
  });
});
