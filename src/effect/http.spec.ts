import { Hono } from "hono";
import { Effect, Schema } from "effect";
import { describe, expect, it, vi } from "vitest";
import { decodeJsonBody, effectHandler, HttpBodyTooLargeError } from "./http.js";

const TEST_BODY_LIMIT = 32;
const BodySchema = Schema.Struct({ value: Schema.String });

function testApp() {
  const app = new Hono();
  app.post(
    "/json",
    effectHandler((c) =>
      decodeJsonBody(c, BodySchema, TEST_BODY_LIMIT).pipe(
        Effect.map((body) => c.json(body)),
        Effect.catchTag("HttpBodyTooLargeError", () =>
          Effect.succeed(c.json({ error: "Request body too large" }, 413)),
        ),
        Effect.catchAll(() => Effect.succeed(c.json({ error: "Invalid body" }, 400))),
      ),
    ),
  );
  return app;
}

function streamingRequest(
  stream: ReadableStream<Uint8Array>,
  headers?: HeadersInit,
): Request {
  return new Request("http://localhost/json", {
    method: "POST",
    headers: { "Content-Type": "application/json", ...headers },
    body: stream,
    duplex: "half",
  } as RequestInit & { duplex: "half" });
}

describe("bounded HTTP JSON bodies", () => {
  it("decodes a valid body under the limit", async () => {
    const response = await testApp().request("/json", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ value: "ok" }),
    });

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual({ value: "ok" });
  });

  it("rejects an oversized declared length", async () => {
    const cancel = vi.fn();
    const response = await testApp().fetch(
      streamingRequest(new ReadableStream({ cancel }), {
        "Content-Length": String(TEST_BODY_LIMIT + 1),
      }),
    );

    expect(response.status).toBe(413);
    expect(cancel).toHaveBeenCalledOnce();
  });

  it("rejects and cancels a chunked body once it crosses the limit", async () => {
    const cancel = vi.fn();
    const response = await testApp().fetch(
      streamingRequest(
        new ReadableStream({
          start(controller) {
            controller.enqueue(new Uint8Array(TEST_BODY_LIMIT));
            controller.enqueue(new Uint8Array(1));
          },
          cancel,
        }),
      ),
    );

    expect(response.status).toBe(413);
    expect(cancel).toHaveBeenCalledOnce();
    await expect(response.json()).resolves.toEqual({
      error: "Request body too large",
    });
  });

  it("keeps oversize failures typed", async () => {
    const app = new Hono();
    let captured: unknown;
    app.post(
      "/json",
      effectHandler((c) =>
        decodeJsonBody(c, BodySchema, TEST_BODY_LIMIT).pipe(
          Effect.map((body) => c.json(body)),
          Effect.catchAll((error) => {
            captured = error;
            return Effect.succeed(c.body(null, 413));
          }),
        ),
      ),
    );

    await app.fetch(
      streamingRequest(
        new ReadableStream({
          start(controller) {
            controller.enqueue(new Uint8Array(TEST_BODY_LIMIT + 1));
          },
        }),
      ),
    );

    expect(captured).toBeInstanceOf(HttpBodyTooLargeError);
  });
});
