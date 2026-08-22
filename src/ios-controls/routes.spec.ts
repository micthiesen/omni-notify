import { createHash, createHmac, randomUUID } from "node:crypto";
import { Injector } from "@micthiesen/mitools/config";
import { Logger, LogLevel } from "@micthiesen/mitools/logging";
import { Hono } from "hono";
import { afterEach, describe, expect, it } from "vitest";
import { IOSControlRegistrationEntity } from "./persistence.js";
import { registerIOSControlRoutes } from "./routes.js";
import { IOSControlService } from "./service.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "ios-control-routes.spec.db",
  },
});

const token = "test-token-that-is-definitely-long-enough";

function app(): Hono {
  const app = new Hono();
  const service = new IOSControlService([], "http://omni.boris", new Logger("Test"));
  registerIOSControlRoutes(app, service, token, new Logger("Test"));
  return app;
}

function authenticatedRequest(path: string, method = "GET", body = "") {
  const timestamp = Math.floor(Date.now() / 1_000).toString();
  const nonce = randomUUID();
  const bodyHash = createHash("sha256").update(body).digest("hex");
  const canonical = `${timestamp}\n${nonce}\n${method}\n${path}\n${bodyHash}`;
  const signature = createHmac("sha256", token).update(canonical).digest("hex");
  return {
    method,
    headers: {
      Authorization: `Omni-HMAC ${signature}`,
      "X-Omni-Timestamp": timestamp,
      "X-Omni-Nonce": nonce,
      ...(body ? { "Content-Type": "application/json" } : {}),
    },
    ...(body ? { body } : {}),
  };
}

afterEach(() => IOSControlRegistrationEntity.deleteAll());

describe("iOS control routes", () => {
  it("requires signed authentication", async () => {
    const response = await app().request("/api/ios-controls/slots/1");
    expect(response.status).toBe(401);
  });

  it("returns a slot using the authenticated wire contract", async () => {
    const response = await app().request(
      "/api/ios-controls/slots/1",
      authenticatedRequest("/api/ios-controls/slots/1"),
    );
    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toMatchObject({
      slot: 1,
      isLive: false,
      displayName: "Nobody Live",
      url: "http://omni.boris",
    });
  });

  it("reports non-secret server diagnostics", async () => {
    const response = await app().request(
      "/api/ios-controls/diagnostics",
      authenticatedRequest("/api/ios-controls/diagnostics"),
    );
    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual({
      apnsEnabled: false,
      registrationCount: 0,
      undeliveredCount: 0,
      lastReconciledAt: null,
    });
  });

  it("validates and persists a complete device registration set", async () => {
    const body = JSON.stringify({
      deviceId: "device-12345",
      controls: [
        {
          controlId: "control-one",
          slot: 1,
          pushToken: "ab".repeat(32),
          environment: "sandbox",
        },
      ],
    });
    const response = await app().request(
      "/api/ios-controls/registrations",
      authenticatedRequest("/api/ios-controls/registrations", "PUT", body),
    );
    expect(response.status).toBe(200);
    expect(IOSControlRegistrationEntity.getAll()).toHaveLength(1);
  });

  it("rejects replayed signed requests", async () => {
    const request = authenticatedRequest("/api/ios-controls/slots/1");
    const server = app();
    expect((await server.request("/api/ios-controls/slots/1", request)).status).toBe(
      200,
    );
    expect((await server.request("/api/ios-controls/slots/1", request)).status).toBe(
      401,
    );
  });
});
