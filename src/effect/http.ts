import type { Context, MiddlewareHandler, Next } from "hono";
import type { EffectRunner } from "@micthiesen/mitools/boundary";
import { Data, Effect, Schema } from "effect";
import { fromPromise } from "./interop.js";

export class HttpBodyError extends Data.TaggedError("HttpBodyError")<{
  readonly cause: unknown;
}> {}

export class HttpBodyTooLargeError extends Data.TaggedError("HttpBodyTooLargeError")<{
  readonly maxBytes: number;
  readonly declaredBytes?: number;
}> {}

export const MAX_JSON_BODY_BYTES = 64 * 1024;

/** The single interpreter used by Hono handlers in this application. */
export function effectHandler<A extends Response, E, R>(
  runner: EffectRunner<R>,
  handler: (context: Context) => Effect.Effect<A, E, R>,
): (context: Context) => Promise<A> {
  return (context) => runner.runPromise(handler(context));
}

/** Adapt Hono's Promise-based middleware continuation at the framework edge. */
export function effectMiddleware<E, R>(
  runner: EffectRunner<R>,
  middleware: (
    context: Context,
    next: Effect.Effect<void, HttpBodyError>,
  ) => Effect.Effect<Response | void, E | HttpBodyError, R>,
): MiddlewareHandler {
  return (context: Context, next: Next) =>
    runner.runPromise(
      middleware(
        context,
        fromPromise("continue Hono middleware", () => next()).pipe(
          Effect.mapError((cause) => new HttpBodyError({ cause })),
        ),
      ),
    );
}

function readBodyBytes(
  body: ReadableStream<Uint8Array>,
  maxBytes: number,
): Effect.Effect<Uint8Array, HttpBodyError | HttpBodyTooLargeError> {
  return Effect.acquireUseRelease(
    Effect.try({
      try: () => body.getReader(),
      catch: (cause) => new HttpBodyError({ cause }),
    }),
    (reader) => {
      const chunks: Uint8Array[] = [];
      let byteLength = 0;

      const read: Effect.Effect<Uint8Array, HttpBodyError | HttpBodyTooLargeError> =
        Effect.suspend(() =>
          Effect.tryPromise({
            try: () => reader.read(),
            catch: (cause) => new HttpBodyError({ cause }),
          }).pipe(
            Effect.flatMap((result) => {
              if (result.done) {
                const bytes = new Uint8Array(byteLength);
                let offset = 0;
                for (const chunk of chunks) {
                  bytes.set(chunk, offset);
                  offset += chunk.byteLength;
                }
                return Effect.succeed(bytes);
              }

              byteLength += result.value.byteLength;
              if (byteLength > maxBytes) {
                return Effect.fail(new HttpBodyTooLargeError({ maxBytes }));
              }
              chunks.push(result.value);
              return read;
            }),
          ),
        );

      return read;
    },
    (reader) =>
      Effect.tryPromise({
        try: () => reader.cancel(),
        catch: () => undefined,
      }).pipe(
        Effect.ignore,
        Effect.andThen(Effect.sync(() => reader.releaseLock()).pipe(Effect.ignore)),
      ),
  );
}

/**
 * Read an untrusted JSON request body without ever buffering more than the
 * configured limit. Content-Length is a fast rejection only; the stream limit
 * remains authoritative for chunked bodies and dishonest clients.
 */
export function readJsonBody(
  context: Context,
  maxBytes = MAX_JSON_BODY_BYTES,
): Effect.Effect<unknown, HttpBodyError | HttpBodyTooLargeError> {
  const declaredLength = context.req.header("Content-Length");
  if (declaredLength && /^\d+$/.test(declaredLength)) {
    const declaredBytes = Number(declaredLength);
    if (declaredBytes > maxBytes) {
      const failure = new HttpBodyTooLargeError({ maxBytes, declaredBytes });
      const body = context.req.raw.body;
      return body
        ? Effect.tryPromise({
            try: () => body.cancel(),
            catch: () => undefined,
          }).pipe(Effect.ignore, Effect.andThen(Effect.fail(failure)))
        : Effect.fail(failure);
    }
  }

  const body = context.req.raw.body;
  const bytesEffect = body
    ? readBodyBytes(body, maxBytes)
    : Effect.succeed(new Uint8Array());

  return bytesEffect.pipe(
    Effect.flatMap((bytes) =>
      Effect.try({
        try: () => JSON.parse(new TextDecoder().decode(bytes)) as unknown,
        catch: (cause) => new HttpBodyError({ cause }),
      }),
    ),
  );
}

/** Read and decode an untrusted JSON request body with Effect Schema. */
export function decodeJsonBody<A, I>(
  context: Context,
  schema: Schema.Codec<A, I>,
  maxBytes = MAX_JSON_BODY_BYTES,
): Effect.Effect<A, HttpBodyError | HttpBodyTooLargeError> {
  return readJsonBody(context, maxBytes).pipe(
    Effect.flatMap((body) =>
      Schema.decodeUnknownEffect(schema)(body).pipe(
        Effect.mapError((cause) => new HttpBodyError({ cause })),
      ),
    ),
  );
}
