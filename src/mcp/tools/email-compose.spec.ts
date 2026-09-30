import { Effect } from "effect";
import { afterAll, describe, expect, it, vi } from "vitest";
import { createMitoolsTestRuntime } from "../../test/mitools.js";

const mocks = vi.hoisted(() => ({
  send: vi.fn(),
  configuration: { from: "sender@example.test" },
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
    expect(createDraftEffect.mock.calls[1]?.[1]).toEqual({ allowAppend: false });
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
