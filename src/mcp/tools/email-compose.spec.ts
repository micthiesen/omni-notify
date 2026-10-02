import { Effect } from "effect";
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
