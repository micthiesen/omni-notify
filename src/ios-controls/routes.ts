import { createHash, createHmac, timingSafeEqual } from "node:crypto";
import type { Logger } from "@micthiesen/mitools/logging";
import type { Hono } from "hono";
import { Clock, Data, Effect, Schema } from "effect";
import {
  decodeJsonBody,
  effectHandler,
  effectMiddleware,
  HttpBodyError,
} from "../effect/http.js";
import { fromPromise } from "../effect/interop.js";
import { IOS_CONTROL_SLOT_COUNT } from "./liveSlots.js";
import type { IOSControlService } from "./service.js";

const boundedString = (min: number, max: number) =>
  Schema.String.pipe(Schema.trimmed(), Schema.minLength(min), Schema.maxLength(max));
const registrationSchema = Schema.Struct({
  deviceId: boundedString(8, 200),
  controls: Schema.Array(
    Schema.Struct({
      controlId: boundedString(1, 500),
      slot: Schema.Number.pipe(Schema.int(), Schema.between(1, IOS_CONTROL_SLOT_COUNT)),
      pushToken: Schema.String.pipe(Schema.pattern(/^[0-9a-fA-F]{32,512}$/)),
      environment: Schema.Literal("sandbox", "production"),
    }),
  ).pipe(Schema.maxItems(32)),
});

const AUTH_WINDOW_SECONDS = 300;
export const IOS_CONTROL_MAX_SIGNED_BODY_BYTES = 64 * 1024;

class IOSControlBodyTooLargeError extends Data.TaggedError(
  "IOSControlBodyTooLargeError",
)<{}> {}

function readBoundedBody(
  request: Request,
): Effect.Effect<
  { body: Buffer; bodyHash: string },
  HttpBodyError | IOSControlBodyTooLargeError
> {
  return Effect.acquireUseRelease(
    Effect.try({
      try: () => request.clone().body?.getReader(),
      catch: (cause) => new HttpBodyError({ cause }),
    }),
    (reader) =>
      Effect.gen(function* () {
        if (!reader) {
          const body = Buffer.alloc(0);
          return {
            body,
            bodyHash: createHash("sha256").update(body).digest("hex"),
          };
        }
        const chunks: Buffer[] = [];
        const hasher = createHash("sha256");
        let size = 0;
        while (true) {
          const result = yield* fromPromise("read signed iOS control body chunk", () =>
            reader.read(),
          ).pipe(Effect.mapError((cause) => new HttpBodyError({ cause })));
          if (result.done) break;
          size += result.value.byteLength;
          if (size > IOS_CONTROL_MAX_SIGNED_BODY_BYTES) {
            return yield* new IOSControlBodyTooLargeError();
          }
          const chunk = Buffer.from(result.value);
          chunks.push(chunk);
          hasher.update(chunk);
        }
        return {
          body: Buffer.concat(chunks, size),
          bodyHash: hasher.digest("hex"),
        };
      }),
    (reader) =>
      reader
        ? fromPromise("release signed iOS control body reader", () =>
            reader.cancel(),
          ).pipe(
            Effect.catchAll(() => Effect.void),
            Effect.ensuring(
              Effect.try({
                try: () => reader.releaseLock(),
                catch: (cause) => new HttpBodyError({ cause }),
              }).pipe(Effect.ignore),
            ),
          )
        : Effect.void,
  );
}

function safeEqual(supplied: string, expected: string): boolean {
  const a = Buffer.from(supplied);
  const b = Buffer.from(expected);
  return a.length === b.length && timingSafeEqual(a, b);
}

function pruneNonces(nonces: Map<string, number>, now: number): void {
  for (const [nonce, expiresAt] of nonces) {
    if (expiresAt <= now) nonces.delete(nonce);
  }
}

export function registerIOSControlRoutes(
  app: Hono,
  service: IOSControlService,
  authToken: string | undefined,
  parentLogger: Logger,
): void {
  const logger = parentLogger.extend("IOSControlRoutes");
  const usedNonces = new Map<string, number>();
  app.use(
    "/api/ios-controls/*",
    effectMiddleware((c, next) =>
      Effect.gen(function* () {
        if (!authToken) {
          return c.json({ error: "iOS controls are not configured" }, 503);
        }
        const contentLengthHeader = c.req.header("Content-Length");
        if (contentLengthHeader !== undefined) {
          const contentLength = Number(contentLengthHeader);
          if (
            !Number.isSafeInteger(contentLength) ||
            contentLength < 0 ||
            contentLength > IOS_CONTROL_MAX_SIGNED_BODY_BYTES
          ) {
            return c.json({ error: "Request body too large" }, 413);
          }
        }
        const timestamp = Number(c.req.header("X-Omni-Timestamp"));
        const nonce = c.req.header("X-Omni-Nonce") ?? "";
        const supplied =
          c.req.header("Authorization")?.replace(/^Omni-HMAC\s+/i, "") ?? "";
        const now = Math.floor((yield* Clock.currentTimeMillis) / 1_000);
        pruneNonces(usedNonces, now);
        if (
          !Number.isInteger(timestamp) ||
          Math.abs(now - timestamp) > AUTH_WINDOW_SECONDS ||
          !/^[0-9a-f-]{36}$/i.test(nonce) ||
          usedNonces.has(nonce)
        ) {
          return c.json({ error: "Unauthorized" }, 401);
        }
        const signedBody = yield* readBoundedBody(c.req.raw).pipe(
          Effect.catchTag("IOSControlBodyTooLargeError", () => Effect.succeed(null)),
        );
        if (signedBody === null) {
          return c.json({ error: "Request body too large" }, 413);
        }
        const path = new URL(c.req.url).pathname;
        const canonical = `${timestamp}\n${nonce}\n${c.req.method}\n${path}\n${signedBody.bodyHash}`;
        const expected = createHmac("sha256", authToken)
          .update(canonical)
          .digest("hex");
        if (!safeEqual(supplied, expected)) {
          return c.json({ error: "Unauthorized" }, 401);
        }
        usedNonces.set(nonce, now + AUTH_WINDOW_SECONDS);
        return yield* next;
      }),
    ),
  );

  app.get("/api/ios-controls/slots/:slot", (c) => {
    const slot = service.getSlot(Number(c.req.param("slot")));
    if (!slot) return c.json({ error: "Slot must be an integer from 1 to 4" }, 400);
    c.header("Cache-Control", "no-store");
    return c.json(slot);
  });

  app.get("/api/ios-controls/diagnostics", (c) => {
    c.header("Cache-Control", "no-store");
    return c.json(service.diagnostics());
  });

  app.put(
    "/api/ios-controls/registrations",
    effectHandler((c) =>
      decodeJsonBody(c, registrationSchema).pipe(
        Effect.flatMap((input) =>
          service
            .registerDeviceEffect(
              input.deviceId,
              input.controls.map((control) => ({ ...control })),
            )
            .pipe(
              Effect.tap(() =>
                Effect.sync(() =>
                  logger.info(
                    `Registered ${input.controls.length} control(s) for one device`,
                  ),
                ),
              ),
              Effect.as(c.json({ registered: input.controls.length })),
            ),
        ),
        Effect.catchAll(() =>
          Effect.succeed(c.json({ error: "Invalid control registration" }, 400)),
        ),
      ),
    ),
  );
}
