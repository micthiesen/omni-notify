import type { Logger } from "@micthiesen/mitools/logging";
import { Effect, Fiber } from "effect";
import type { JamClient } from "jmap-jam";
import { describe, expect, it, vi } from "vitest";
import type { EmailAttachment } from "../types.js";
import type { JmapContext } from "./client.js";
import {
  downloadJmapAttachmentEffect,
  JmapTransportError,
  MAX_JMAP_ATTACHMENT_BYTES,
  readJmapAttachmentBodyEffect,
} from "./transport.js";

const logger = {
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
  extend: vi.fn(),
} as unknown as Logger;

function attachment(size: number): EmailAttachment {
  return {
    blobId: "blob-1",
    name: "document.pdf",
    type: "application/pdf",
    size,
  };
}

function contextWithDownload(downloadBlob: JamClient["downloadBlob"]): JmapContext {
  return {
    accountId: "account-1",
    jam: { downloadBlob } as unknown as JamClient,
  };
}

function chunkedResponse(
  chunks: Uint8Array[],
  cancel = vi.fn(),
  headers?: HeadersInit,
): { response: Response; cancel: ReturnType<typeof vi.fn> } {
  return {
    response: new Response(
      new ReadableStream<Uint8Array>({
        start(controller) {
          for (const chunk of chunks) controller.enqueue(chunk);
          controller.close();
        },
        cancel,
      }),
      { headers },
    ),
    cancel,
  };
}

describe("JMAP attachment downloads", () => {
  it("uses the streamed byte count instead of rejecting false large metadata", async () => {
    const { response } = chunkedResponse([new Uint8Array([1, 2, 3])]);
    const downloadBlob = vi.fn(() => Promise.resolve(response));

    const result = await Effect.runPromise(
      downloadJmapAttachmentEffect(
        contextWithDownload(downloadBlob),
        logger,
        attachment(MAX_JMAP_ATTACHMENT_BYTES + 1),
      ),
    );

    expect(result?.data).toEqual(Buffer.from([1, 2, 3]));
  });

  it.each([
    ["false small metadata", 1],
    ["missing metadata", undefined],
  ])("rejects chunked overflow with %s", async (_description, size) => {
    const cancel = vi.fn();
    const chunks = [new Uint8Array(3 * 1024 * 1024), new Uint8Array(3 * 1024 * 1024)];
    let chunkIndex = 0;
    const response = new Response(
      new ReadableStream<Uint8Array>({
        pull(controller) {
          controller.enqueue(chunks[chunkIndex++]);
        },
        cancel,
      }),
      { headers: { "content-length": "1" } },
    );
    const downloadBlob = vi.fn(() => Promise.resolve(response));

    const result = await Effect.runPromise(
      downloadJmapAttachmentEffect(
        contextWithDownload(downloadBlob),
        logger,
        attachment(size as number),
      ),
    );

    expect(result).toBeUndefined();
    expect(cancel).toHaveBeenCalledOnce();
    expect(logger.warn).toHaveBeenCalledWith(
      'Error downloading "document.pdf"',
      expect.stringContaining(`${MAX_JMAP_ATTACHMENT_BYTES} byte limit`),
    );
  });

  it("keeps streamed read failures typed", async () => {
    const { response } = chunkedResponse([
      new Uint8Array(MAX_JMAP_ATTACHMENT_BYTES + 1),
    ]);

    const error = await Effect.runPromise(
      readJmapAttachmentBodyEffect(response).pipe(Effect.flip),
    );

    expect(error).toBeInstanceOf(JmapTransportError);
    expect(error.operation).toBe("read attachment");
  });

  it("aborts the request and cancels the body reader when interrupted", async () => {
    const cancel = vi.fn();
    const pull = vi.fn();
    const response = new Response(new ReadableStream<Uint8Array>({ pull, cancel }));
    let requestSignal: AbortSignal | undefined;
    const downloadBlob = vi.fn(
      (_options: unknown, init?: RequestInit): Promise<Response> => {
        requestSignal = init?.signal ?? undefined;
        return Promise.resolve(response);
      },
    );
    const fiber = Effect.runFork(
      downloadJmapAttachmentEffect(
        contextWithDownload(downloadBlob as JamClient["downloadBlob"]),
        logger,
        attachment(0),
      ),
    );
    await vi.waitFor(() => expect(pull).toHaveBeenCalled());

    await Effect.runPromise(Fiber.interrupt(fiber));

    expect(requestSignal?.aborted).toBe(true);
    expect(cancel).toHaveBeenCalledOnce();
  });
});
