import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { Clock, Data, Effect } from "effect";
import {
  assertPublicHttpUrlSyntax,
  publicGotStream,
  PUBLIC_HTTP_USER_AGENT,
  readTextResponseWithLimit,
} from "../../effect/publicHttp.js";

export class WebhookError extends Data.TaggedError("WebhookError")<{
  readonly reason:
    | "invalid_callback"
    | "invalid_secret"
    | "challenge_failed"
    | "timeout"
    | "delivery_failed";
}> {
  override get message() {
    return `Event webhook ${this.reason}`;
  }
}

export function validateCallbackUrl(value: string): URL {
  const url = assertPublicHttpUrlSyntax(value);
  if (url.protocol !== "https:" || url.hash || value.length > 2048) {
    throw new Error("Callback requires a bounded HTTPS URL without a fragment");
  }
  return url;
}

export function validateSigningSecret(value: string): Buffer {
  if (!/^whsec_[A-Za-z0-9+/]+={0,2}$/.test(value))
    throw new Error("Invalid signing secret");
  const encoded = value.slice(6);
  const key = Buffer.from(encoded, "base64");
  if (
    key.length < 24 ||
    key.length > 64 ||
    key.toString("base64").replace(/=+$/, "") !== encoded.replace(/=+$/, "")
  ) {
    throw new Error("Invalid signing secret");
  }
  return key;
}

export interface WebhookDestination {
  id: string;
  url: string;
  secret: string;
  previousSecret?: string;
}

export interface WebhookEvent {
  eventId: string;
  name: string;
  timestamp: string;
  data: unknown;
  cursor: null;
}

export function webhookHeaders(
  destination: WebhookDestination,
  id: string,
  seconds: number,
  body: string,
): Record<string, string> {
  const signature = (secret: string) =>
    `v1,${createHmac("sha256", validateSigningSecret(secret)).update(`${id}.${seconds}.${body}`).digest("base64")}`;
  return {
    "content-type": "application/json",
    "user-agent": PUBLIC_HTTP_USER_AGENT,
    "webhook-id": id,
    "webhook-timestamp": String(seconds),
    "webhook-signature": [
      signature(destination.secret),
      ...(destination.previousSecret ? [signature(destination.previousSecret)] : []),
    ].join(" "),
    "X-MCP-Subscription-Id": destination.id,
  };
}

export type WebhookRequest = (
  url: string,
  body: string,
  headers: Record<string, string>,
) => Effect.Effect<{ status: number; body: string }, WebhookError>;

/** The validated DNS result is used by the actual socket lookup; redirects and
 * pooled connections are disabled. TLS still verifies the original hostname. */
const requestWebhook: WebhookRequest = (url, body, headers) =>
  Effect.tryPromise({
    try: async (signal) => {
      validateCallbackUrl(url);
      const response = publicGotStream(url, {
        method: "POST",
        body,
        headers,
        signal,
        followRedirect: false,
        throwHttpErrors: false,
        retry: { limit: 0 },
        timeout: { request: 10_000 },
        agent: { https: false },
      });
      const text = await readTextResponseWithLimit(response, 4096);
      const metadata = response.response as { statusCode?: number } | undefined;
      return { status: metadata?.statusCode ?? 0, body: text };
    },
    catch: (error) =>
      new WebhookError({
        reason:
          error instanceof Error && error.name === "TimeoutError"
            ? "timeout"
            : "delivery_failed",
      }),
  });

export function createWebhookClient(request: WebhookRequest = requestWebhook) {
  const post = (destination: WebhookDestination, id: string, value: unknown) =>
    Effect.gen(function* () {
      yield* Effect.try({
        try: () => validateCallbackUrl(destination.url),
        catch: () => new WebhookError({ reason: "invalid_callback" }),
      });
      const seconds = Math.floor((yield* Clock.currentTimeMillis) / 1000);
      const body = JSON.stringify(value);
      if (Buffer.byteLength(body) > 262_144)
        return yield* new WebhookError({ reason: "delivery_failed" });
      const headers = yield* Effect.try({
        try: () => webhookHeaders(destination, id, seconds, body),
        catch: () => new WebhookError({ reason: "invalid_secret" }),
      });
      return yield* request(destination.url, body, headers);
    });
  return {
    verifyCallback: (
      destination: WebhookDestination,
    ): Effect.Effect<void, WebhookError> =>
      Effect.gen(function* () {
        const challenge = yield* Effect.sync(() =>
          randomBytes(32).toString("base64url"),
        );
        const id = yield* Effect.sync(
          () => `msg_verification_${randomBytes(16).toString("hex")}`,
        );
        const response = yield* post(destination, id, {
          type: "verification",
          challenge,
        });
        const valid = yield* Effect.try({
          try: () => {
            const parsed: unknown = JSON.parse(response.body);
            const echo =
              typeof parsed === "object" && parsed !== null && "challenge" in parsed
                ? parsed.challenge
                : undefined;
            return (
              response.status >= 200 &&
              response.status < 300 &&
              typeof echo === "string" &&
              Buffer.byteLength(echo) === Buffer.byteLength(challenge) &&
              timingSafeEqual(Buffer.from(echo), Buffer.from(challenge))
            );
          },
          catch: () => new WebhookError({ reason: "challenge_failed" }),
        });
        if (!valid) return yield* new WebhookError({ reason: "challenge_failed" });
      }),
    deliverWebhook: (input: WebhookDestination & { event: WebhookEvent }) =>
      post(input, input.event.eventId, input.event).pipe(
        Effect.map(({ status }) => ({ status })),
      ),
  };
}

export const { verifyCallback, deliverWebhook } = createWebhookClient();
