import type { EffectRunner } from "@micthiesen/mitools/boundary";
import type { Hono } from "hono";
import { Clock, Effect, Schema } from "effect";
import { effectHandler, effectMiddleware, decodeJsonBody } from "../effect/http.js";

const diagnosticStages = [
  "sign-in-init",
  "sign-in-proof",
  "sign-in-complete",
  "account-session",
  "second-factor",
  "apple-request",
  "private-storage",
] as const;
const diagnosticCategories = [
  "apple-response",
  "transport",
  "protocol",
  "storage",
  "authentication",
] as const;

export type RemindersDiagnostic = {
  stage: (typeof diagnosticStages)[number];
  category: (typeof diagnosticCategories)[number];
  httpStatus?: number;
};

export type RemindersPublicStatus = {
  enabled: boolean;
  phase:
    | "disabled"
    | "authenticated"
    | "authentication-needed"
    | "transient-outage"
    | "rate-limited"
    | "unsupported-protocol"
    | "awaiting-device-approval"
    | "terms-required";
  reason?:
    | "mfa"
    | "pcs"
    | "terms"
    | "credentials"
    | "session-expired"
    | "configuration"
    | "protocol";
  challengeId?: string;
  challengeExpiresAt?: number;
  diagnostic?: RemindersDiagnostic;
};

export interface RemindersControl<R = never> {
  status(): Effect.Effect<RemindersPublicStatus, unknown, R>;
  startAuthentication(): Effect.Effect<RemindersPublicStatus, unknown, R>;
  submitCode(input: {
    challengeId: string;
    code: string;
  }): Effect.Effect<RemindersPublicStatus, unknown, R>;
  verifyAccess(): Effect.Effect<RemindersPublicStatus, unknown, R>;
}

const codeSchema = Schema.Struct({
  challengeId: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(200)),
  code: Schema.String.check(Schema.isPattern(/^[0-9]{6}$/)),
});

function publicStatus(status: RemindersPublicStatus): RemindersPublicStatus {
  const diagnostic = status.diagnostic;
  return {
    enabled: status.enabled,
    phase: status.phase,
    ...(status.reason ? { reason: status.reason } : {}),
    ...(status.challengeId ? { challengeId: status.challengeId } : {}),
    ...(status.challengeExpiresAt
      ? { challengeExpiresAt: status.challengeExpiresAt }
      : {}),
    ...(diagnostic &&
    diagnosticStages.includes(diagnostic.stage) &&
    diagnosticCategories.includes(diagnostic.category)
      ? {
          diagnostic: {
            stage: diagnostic.stage,
            category: diagnostic.category,
            ...(Number.isInteger(diagnostic.httpStatus) &&
            diagnostic.httpStatus! >= 100 &&
            diagnostic.httpStatus! <= 599
              ? { httpStatus: diagnostic.httpStatus }
              : {}),
          },
        }
      : {}),
  };
}

const limits = {
  "/api/reminders/auth/start": { count: 3, windowMs: 5 * 60_000 },
  "/api/reminders/auth/code": { count: 10, windowMs: 60_000 },
  "/api/reminders/auth/verify": { count: 10, windowMs: 60_000 },
} as const;

/** Public status and challenge controls; no account data is returned here. */
export function registerRemindersRoutes<R>(
  runner: EffectRunner<R>,
  app: Hono,
  control: RemindersControl<R>,
  publicOrigin: string | undefined,
): void {
  const attempts = new Map<string, number[]>();
  let allowedOrigin: string | undefined;
  try {
    const parsed = new URL(publicOrigin ?? "");
    if (
      parsed.protocol === "https:" &&
      !parsed.username &&
      !parsed.password &&
      parsed.pathname === "/" &&
      !parsed.search &&
      !parsed.hash
    ) {
      allowedOrigin = parsed.origin;
    }
  } catch {
    // Invalid configuration fails closed below.
  }

  app.use(
    "/api/reminders/*",
    effectMiddleware(runner, (c, next) =>
      Effect.gen(function* () {
        c.header("Cache-Control", "no-store");
        c.header("Pragma", "no-cache");
        c.header("X-Content-Type-Options", "nosniff");
        if (!allowedOrigin) {
          if (c.req.method === "GET" && c.req.path === "/api/reminders/status") {
            return c.json({
              status: { enabled: false, phase: "disabled", reason: "configuration" },
            });
          }
          return c.json({ error: "Reminders administration is unavailable" }, 503);
        }
        const host = c.req.header("Host") ?? new URL(c.req.url).host;
        if (host !== new URL(allowedOrigin).host) {
          return c.json({ error: "Forbidden" }, 403);
        }
        if (c.req.method !== "GET") {
          if (
            c.req.header("Origin") !== allowedOrigin ||
            (c.req.header("Sec-Fetch-Site") &&
              c.req.header("Sec-Fetch-Site") !== "same-origin")
          ) {
            return c.json({ error: "Forbidden" }, 403);
          }
          if (
            c.req.header("Content-Type")?.split(";", 1)[0]?.trim() !==
            "application/json"
          ) {
            return c.json({ error: "Expected application/json" }, 415);
          }
          const limit = limits[c.req.path as keyof typeof limits];
          if (limit) {
            const now = yield* Clock.currentTimeMillis;
            const recent = (attempts.get(c.req.path) ?? []).filter(
              (time) => now - time < limit.windowMs,
            );
            if (recent.length >= limit.count) {
              c.header("Retry-After", String(Math.ceil(limit.windowMs / 1000)));
              return c.json({ error: "Too many requests" }, 429);
            }
            recent.push(now);
            attempts.set(c.req.path, recent);
          }
        }
        yield* next;
        c.header("Cache-Control", "no-store");
        c.header("Pragma", "no-cache");
        c.header("X-Content-Type-Options", "nosniff");
      }),
    ),
  );

  app.get(
    "/api/reminders/status",
    effectHandler(runner, (c) =>
      control.status().pipe(
        Effect.map((status) => c.json({ status: publicStatus(status) })),
        Effect.catch(() =>
          Effect.succeed(c.json({ error: "Reminders request failed" }, 502)),
        ),
      ),
    ),
  );
  app.post(
    "/api/reminders/auth/start",
    effectHandler(runner, (c) =>
      control.startAuthentication().pipe(
        Effect.map((status) => c.json({ status: publicStatus(status) })),
        Effect.catch(() =>
          control.status().pipe(
            Effect.map((status) =>
              c.json(
                { error: "Reminders request failed", status: publicStatus(status) },
                status.phase === "rate-limited" ? 429 : 502,
              ),
            ),
            Effect.catch(() =>
              Effect.succeed(c.json({ error: "Reminders request failed" }, 502)),
            ),
          ),
        ),
      ),
    ),
  );
  app.post(
    "/api/reminders/auth/code",
    effectHandler(runner, (c) =>
      decodeJsonBody(c, codeSchema, 512).pipe(
        Effect.flatMap((input) => control.submitCode(input)),
        Effect.map((status) => c.json({ status: publicStatus(status) })),
        Effect.catch(() =>
          Effect.succeed(c.json({ error: "Code submission was not confirmed" }, 400)),
        ),
      ),
    ),
  );
  app.post(
    "/api/reminders/auth/verify",
    effectHandler(runner, (c) =>
      control.verifyAccess().pipe(
        Effect.map((status) => c.json({ status: publicStatus(status) })),
        Effect.catch(() =>
          control.status().pipe(
            Effect.map((status) =>
              c.json(
                { error: "Reminders request failed", status: publicStatus(status) },
                status.phase === "rate-limited" ? 429 : 502,
              ),
            ),
            Effect.catch(() =>
              Effect.succeed(c.json({ error: "Reminders request failed" }, 502)),
            ),
          ),
        ),
      ),
    ),
  );
}
