import { Effect } from "effect";
import { afterAll, describe, expect, it, vi } from "vitest";
import { createMitoolsTestRuntime } from "../../test/mitools.js";
import type { McpRuntime } from "../runtime.js";
import { createEmailAttachmentTools } from "./email-attachments.js";

const runtime = createMitoolsTestRuntime();
afterAll(() => runtime.dispose());
const messageId = "<attachment@example.test>";
const data = Buffer.from("%PDF-1.7\nsynthetic fixture");

function fixture(
  result: { name: string; mimeType: string; data: Buffer } | undefined = {
    name: "../private.pdf",
    mimeType: "application/pdf",
    data,
  },
) {
  const fetch = vi.fn((): Effect.Effect<typeof result | undefined> =>
    Effect.succeed(result),
  );
  const [tool] = createEmailAttachmentTools({
    emailControls: { transport: { fetchAttachmentByIdEffect: fetch } },
  } as unknown as McpRuntime);
  return { tool, fetch };
}

describe("private email PDF MCP retrieval", () => {
  it("returns an embedded binary resource and safe metadata without a public URL", async () => {
    const { tool, fetch } = fixture();
    const value = await runtime.run(
      tool.execute({ messageId, attachmentId: "stable-id" }),
    );
    expect(fetch).toHaveBeenCalledExactlyOnceWith(messageId, "stable-id", {
      maxBytes: 5 * 1024 * 1024,
    });
    const formatted = tool.formatResult!(value);
    expect(formatted.structuredContent).toEqual(value);
    expect(formatted.content[0]).toMatchObject({ type: "text" });
    expect(JSON.stringify(formatted.content[0])).not.toContain(data.toString("base64"));
    expect(formatted.content[1]).toEqual({
      type: "resource",
      resource: {
        uri: "omni-email-attachment:stable-id",
        mimeType: "application/pdf",
        blob: data.toString("base64"),
      },
    });
    expect(value.filename).not.toMatch(/[\\/\x00-\x1f]/);
    expect(value.size).toBe(data.length);
    expect(value.sha256).toMatch(/^[a-f0-9]{64}$/);
  });

  it("rejects excessive bounds and header injection before touching IMAP", async () => {
    const { tool, fetch } = fixture();
    for (const input of [
      { messageId, attachmentId: "id", maxBytes: 5 * 1024 * 1024 + 1 },
      { messageId: "<id>\r\nInjected: header", attachmentId: "id" },
      { messageId, attachmentId: "id", destination: "/tmp/file.pdf" },
    ])
      await expect(runtime.run(tool.execute(input))).rejects.toThrow();
    expect(fetch).not.toHaveBeenCalled();
  });

  it("rejects missing parts, mismatched MIME, invalid PDF bytes and decoded oversize", async () => {
    for (const result of [
      undefined,
      { name: "x.pdf", mimeType: "text/html", data },
      { name: "x.pdf", mimeType: "application/pdf", data: Buffer.from("not pdf") },
    ]) {
      const { tool, fetch } = fixture();
      fetch.mockReturnValueOnce(Effect.succeed(result));
      await expect(
        runtime.run(tool.execute({ messageId, attachmentId: "id" })),
      ).rejects.toThrow();
    }
    const { tool } = fixture();
    await expect(
      runtime.run(tool.execute({ messageId, attachmentId: "id", maxBytes: 5 })),
    ).rejects.toThrow(/limit/);
  });
});
