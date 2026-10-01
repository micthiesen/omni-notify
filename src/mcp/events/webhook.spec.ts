import { createHmac } from "node:crypto";
import { Effect } from "effect";
import { describe, expect, it } from "@effect/vitest";
import {
  createWebhookClient,
  validateCallbackUrl,
  validateSigningSecret,
  webhookHeaders,
} from "./webhook.js";

const secret = `whsec_${Buffer.alloc(32, 7).toString("base64")}`;
const destination = { id: "sub_fixture", url: "https://example.com/callback", secret };

describe("event webhook contract", () => {
  it("matches Standard Webhooks exact body bytes and rotates keys", () => {
    const body = '{"hello":"world"}';
    const old = `whsec_${Buffer.alloc(24, 8).toString("base64")}`;
    const headers = webhookHeaders(
      { ...destination, previousSecret: old },
      "evt_fixture",
      123,
      body,
    );
    const expected = createHmac("sha256", Buffer.alloc(32, 7))
      .update(`evt_fixture.123.${body}`)
      .digest("base64");
    expect(headers["webhook-signature"].split(" ")[0]).toBe(`v1,${expected}`);
    expect(headers["webhook-signature"].split(" ")).toHaveLength(2);
    expect(headers["webhook-id"]).toBe("evt_fixture");
    expect(headers["X-MCP-Subscription-Id"]).toBe(destination.id);
  });
  it("rejects invalid secrets and nonpublic/non-HTTPS callback syntax", () => {
    for (const value of [
      "plain",
      "whsec_a",
      `whsec_${Buffer.alloc(23).toString("base64")}`,
      `whsec_${Buffer.alloc(65).toString("base64")}`,
    ])
      expect(() => validateSigningSecret(value)).toThrow();
    for (const value of [
      "http://example.com",
      "https://localhost",
      "https://127.1",
      "https://[::ffff:127.0.0.1]",
      "https://example.com/#secret",
      "https://user:pass@example.com",
      "https://[fec0::1]",
    ])
      expect(() => validateCallbackUrl(value)).toThrow();
  });
  it.effect(
    "verifies a fresh challenge, signs it, and never sends mail during verification",
    () =>
      Effect.gen(function* () {
        const challenges: string[] = [];
        const client = createWebhookClient((_url, body, headers) => {
          const value = JSON.parse(body);
          expect(value.type).toBe("verification");
          expect(Object.keys(value).sort()).toEqual(["challenge", "type"]);
          expect(headers["webhook-id"]).toMatch(/^msg_verification_/);
          challenges.push(value.challenge);
          return Effect.succeed({
            status: 200,
            body: JSON.stringify({ challenge: value.challenge }),
          });
        });
        yield* client.verifyCallback(destination);
        yield* client.verifyCallback(destination);
        expect(challenges[0]).not.toBe(challenges[1]);
      }),
  );
  it.effect("rejects wrong echo, redirects and unsuccessful verification", () =>
    Effect.gen(function* () {
      for (const status of [200, 302, 500]) {
        const client = createWebhookClient(() =>
          Effect.succeed({ status, body: '{"challenge":"wrong"}' }),
        );
        const result = yield* client.verifyCallback(destination).pipe(Effect.result);
        expect(result._tag).toBe("Failure");
      }
    }),
  );
  it.effect("preserves the event ID and exact payload on delivery", () =>
    Effect.gen(function* () {
      const event = {
        eventId: "evt_fixture",
        name: "email.received",
        timestamp: "2026-10-01T00:00:00Z",
        data: { messageId: "fixture" },
        cursor: null,
      };
      const client = createWebhookClient((_url, body, headers) => {
        expect(JSON.parse(body)).toEqual(event);
        expect(headers["webhook-id"]).toBe(event.eventId);
        return Effect.succeed({ status: 410, body: "" });
      });
      expect(yield* client.deliverWebhook({ ...destination, event })).toEqual({
        status: 410,
      });
    }),
  );
});
