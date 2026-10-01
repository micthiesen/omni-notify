import { Effect } from "effect";
import { Hono } from "hono";
import { describe, expect, it, vi } from "vitest";
import { testRuntime } from "../live-check/testRuntime.js";
import {
  registerRemindersRoutes,
  type RemindersControl,
  type RemindersPublicStatus,
} from "./routes.js";

const origin = "https://omni.example.test";
const initial = { enabled: true, phase: "authentication-needed" as const };

function fixture(config?: { origin?: string }) {
  const status = vi.fn<RemindersControl["status"]>(() => Effect.succeed(initial));
  const startAuthentication = vi.fn<RemindersControl["startAuthentication"]>(() =>
    Effect.succeed({ ...initial, challengeId: "challenge-one" }),
  );
  const submitCode = vi.fn(() =>
    Effect.succeed({ enabled: true, phase: "authenticated" as const }),
  );
  const verifyAccess = vi.fn<RemindersControl["verifyAccess"]>(() =>
    Effect.succeed(initial),
  );
  const control: RemindersControl = {
    status,
    startAuthentication,
    submitCode,
    verifyAccess,
  };
  const app = new Hono();
  registerRemindersRoutes(testRuntime, app, control, config?.origin ?? origin);
  const request = (
    path: string,
    method = "GET",
    headers: Record<string, string> = {},
    body?: string,
  ) =>
    app.request(`${origin}${path}`, {
      method,
      headers: {
        Host: "omni.example.test",
        ...(method === "GET"
          ? {}
          : { Origin: origin, "Content-Type": "application/json" }),
        ...headers,
      },
      body,
    });
  return { request, status, startAuthentication, submitCode, verifyAccess };
}

describe("Reminders admin routes", () => {
  it("includes safe diagnostic status in failed start and verify responses", async () => {
    const x = fixture();
    const diagnostic = {
      stage: "sign-in-init",
      category: "apple-response",
      httpStatus: 503,
    };
    x.status.mockImplementation(() =>
      Effect.succeed({
        enabled: true,
        phase: "transient-outage",
        diagnostic: {
          ...diagnostic,
          reason: "secret reason",
          cookies: "secret cookies",
        },
        sessionToken: "secret token",
      } as RemindersPublicStatus),
    );
    x.startAuthentication.mockImplementation(() => Effect.fail(new Error("secret")));
    x.verifyAccess.mockImplementation(() => Effect.fail(new Error("secret")));
    for (const action of ["start", "verify"]) {
      const response = await x.request(
        `/api/reminders/auth/${action}`,
        "POST",
        {},
        "{}",
      );
      expect(response.status).toBe(502);
      expect(await response.json()).toEqual({
        error: "Reminders request failed",
        status: { enabled: true, phase: "transient-outage", diagnostic },
      });
    }
  });

  it("rejects arbitrary diagnostic labels and invalid status codes", async () => {
    const x = fixture();
    x.status.mockImplementationOnce(() =>
      Effect.succeed({
        ...initial,
        diagnostic: { stage: "secret URL", category: "transport", httpStatus: 503 },
      } as unknown as RemindersPublicStatus),
    );
    expect(await (await x.request("/api/reminders/status")).json()).toEqual({
      status: initial,
    });
    x.status.mockImplementationOnce(() =>
      Effect.succeed({
        ...initial,
        diagnostic: { stage: "sign-in-init", category: "transport", httpStatus: 900 },
      }),
    );
    expect(await (await x.request("/api/reminders/status")).json()).toEqual({
      status: {
        ...initial,
        diagnostic: { stage: "sign-in-init", category: "transport" },
      },
    });
  });

  it("returns 429 when authentication is in its service cooldown", async () => {
    const x = fixture();
    x.status.mockImplementation(() =>
      Effect.succeed({ enabled: true, phase: "rate-limited" }),
    );
    x.startAuthentication.mockImplementation(() => Effect.fail(new Error("limited")));
    const response = await x.request("/api/reminders/auth/start", "POST", {}, "{}");
    expect(response.status).toBe(429);
    expect(await response.json()).toEqual({
      error: "Reminders request failed",
      status: { enabled: true, phase: "rate-limited" },
    });
  });

  it("returns only public status metadata with no-store headers", async () => {
    const x = fixture();
    const valid = await x.request("/api/reminders/status");
    expect(valid.status).toBe(200);
    expect(valid.headers.get("Cache-Control")).toBe("no-store");
    await expect(valid.json()).resolves.toEqual({ status: initial });
  });

  it("rejects mutations with missing or mismatched Origin", async () => {
    const x = fixture();
    for (const suppliedOrigin of [
      "",
      "https://evil.example.test",
      "http://omni.example.test",
    ]) {
      const response = await x.request(
        "/api/reminders/auth/start",
        "POST",
        {
          Origin: suppliedOrigin,
        },
        "{}",
      );
      expect(response.status).toBe(403);
    }
    expect(x.startAuthentication).not.toHaveBeenCalled();
  });

  it("rejects cross-site fetch metadata", async () => {
    const x = fixture();
    const response = await x.request(
      "/api/reminders/auth/start",
      "POST",
      {
        "Sec-Fetch-Site": "cross-site",
      },
      "{}",
    );
    expect(response.status).toBe(403);
    expect(x.startAuthentication).not.toHaveBeenCalled();
  });

  it("rejects a mismatched host and insecure configuration", async () => {
    const x = fixture();
    expect(
      (await x.request("/api/reminders/status", "GET", { Host: "evil.example.test" }))
        .status,
    ).toBe(403);
    const insecure = fixture({ origin: "http://omni.example.test" });
    expect(await (await insecure.request("/api/reminders/status")).json()).toEqual({
      status: { enabled: false, phase: "disabled", reason: "configuration" },
    });
    expect(
      (await insecure.request("/api/reminders/auth/start", "POST", {}, "{}")).status,
    ).toBe(503);
    expect(insecure.status).not.toHaveBeenCalled();
  });

  it("bounds and validates the verification code before calling the service", async () => {
    const x = fixture();
    const malformed = await x.request(
      "/api/reminders/auth/code",
      "POST",
      {},
      JSON.stringify({ challengeId: "challenge-one", code: "1234567" }),
    );
    expect(malformed.status).toBe(400);
    expect(x.submitCode).not.toHaveBeenCalled();
    const valid = await x.request(
      "/api/reminders/auth/code",
      "POST",
      {},
      JSON.stringify({ challengeId: "challenge-one", code: "123456" }),
    );
    expect(valid.status).toBe(200);
    expect(x.submitCode).toHaveBeenCalledWith({
      challengeId: "challenge-one",
      code: "123456",
    });
  });

  it("does not return service errors to the client", async () => {
    const x = fixture();
    x.startAuthentication.mockImplementationOnce(() =>
      Effect.fail(new Error("secret upstream session token")),
    );
    const response = await x.request("/api/reminders/auth/start", "POST", {}, "{}");
    expect(response.status).toBe(502);
    expect(await response.text()).not.toContain("secret upstream session token");
  });

  it("bounds repeated authentication starts", async () => {
    const x = fixture();
    for (let i = 0; i < 3; i++) {
      expect(
        (await x.request("/api/reminders/auth/start", "POST", {}, "{}")).status,
      ).toBe(200);
    }
    const limited = await x.request("/api/reminders/auth/start", "POST", {}, "{}");
    expect(limited.status).toBe(429);
    expect(limited.headers.get("Cache-Control")).toBe("no-store");
    expect(x.startAuthentication).toHaveBeenCalledTimes(3);
  });
});
