import type { EffectRunner } from "@micthiesen/mitools/boundary";
import type { Hono } from "hono";
import { Effect, Schema } from "effect";
import { decodeJsonBody, effectHandler } from "../effect/http.js";
import { hasValidBearerToken, unauthorizedMcpResponse } from "../mcp/auth.js";
import type { DeviceJobOutcome, DeviceLinkService } from "./service.js";

export const DEVICE_RESULT_MAX_BYTES = 512 * 1024;

const pollSchema = Schema.Struct({
  v: Schema.Literal(1),
  disabled: Schema.Boolean,
  host: Schema.optional(Schema.String.check(Schema.isMaxLength(200))),
  agentVersion: Schema.optional(Schema.String.check(Schema.isMaxLength(40))),
});

const resultSchema = Schema.Struct({
  v: Schema.Literal(1),
  id: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(100)),
  output: Schema.optional(Schema.Unknown),
  error: Schema.optional(
    Schema.Struct({
      code: Schema.String.check(Schema.isMaxLength(100)),
      message: Schema.String.check(Schema.isMaxLength(4_000)),
    }),
  ),
});

/** Resolves when the client goes away so a held poll stops claiming jobs. */
function aborted(signal: AbortSignal): Effect.Effect<never[]> {
  return Effect.callback<never[]>((resume) => {
    if (signal.aborted) return resume(Effect.succeed([]));
    const onAbort = () => resume(Effect.succeed([]));
    signal.addEventListener("abort", onAbort, { once: true });
    return Effect.sync(() => signal.removeEventListener("abort", onAbort));
  });
}

/** Mac-facing long-poll endpoints, authenticated by the device token only. */
export function registerDeviceLinkRoutes<R>(
  runner: EffectRunner<R>,
  app: Hono,
  service: DeviceLinkService,
  token: string,
): void {
  app.post(
    "/device-link/poll",
    effectHandler(runner, (c) =>
      Effect.gen(function* () {
        if (!hasValidBearerToken(c.req.header("Authorization"), token)) {
          return unauthorizedMcpResponse();
        }
        const body = yield* decodeJsonBody(c, pollSchema);
        const jobs = yield* service
          .poll({ disabled: body.disabled, host: body.host ?? null })
          .pipe(Effect.raceFirst(aborted(c.req.raw.signal)));
        c.header("Cache-Control", "no-store");
        return c.json({ v: 1, jobs });
      }).pipe(
        Effect.catch(() => Effect.succeed(c.json({ error: "Bad request" }, 400))),
      ),
    ),
  );

  app.post(
    "/device-link/result",
    effectHandler(runner, (c) =>
      Effect.gen(function* () {
        if (!hasValidBearerToken(c.req.header("Authorization"), token)) {
          return unauthorizedMcpResponse();
        }
        const body = yield* decodeJsonBody(c, resultSchema, DEVICE_RESULT_MAX_BYTES);
        const outcome: DeviceJobOutcome = body.error
          ? { kind: "error", code: body.error.code, message: body.error.message }
          : { kind: "output", output: body.output };
        const accepted = yield* service.complete(body.id, outcome);
        c.header("Cache-Control", "no-store");
        return c.json({ v: 1, accepted });
      }).pipe(
        Effect.catch(() => Effect.succeed(c.json({ error: "Bad request" }, 400))),
      ),
    ),
  );
}
