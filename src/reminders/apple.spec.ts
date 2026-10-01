import { describe, expect, it, vi } from "vitest";
import { Effect, Result } from "effect";
import { AppleRemindersClient, type AppleSession } from "./apple.js";

const saved: AppleSession = {
  clientId: "auth-test",
  sessionToken: "session",
  trustToken: "trust",
  accountCountry: "US",
  ckBaseUrl:
    "https://ckdatabasews.icloud.com/database/1/com.apple.reminders/production/private",
};

function client(
  responder: (url: URL, init: RequestInit) => Response | Promise<Response>,
  initial: AppleSession = saved,
) {
  const writes: AppleSession[] = [];
  const fetchMock = vi.fn(responder);
  const instance = new AppleRemindersClient({
    account: "test@example.com",
    password: "test-password",
    loadSession: () => Effect.succeed(initial),
    saveSession: (session) =>
      Effect.sync(() => {
        writes.push(session);
      }),
    fetch: fetchMock as typeof fetch,
  });
  return { instance, writes, fetchMock };
}

describe("Apple Reminders transport", () => {
  it("uses upstream service headers for setup and CloudKit only", async () => {
    const { instance, fetchMock } = client((url) =>
      Response.json(
        url.pathname.endsWith("/validate")
          ? {
              dsInfo: { hsaVersion: 2 },
              hsaTrustedBrowser: true,
              webservices: { ckdatabasews: { url: "https://ckdatabasews.icloud.com" } },
            }
          : { zones: [{ records: [] }] },
      ),
    );
    expect(await Effect.runPromise(instance.verify())).toBe(true);
    await Effect.runPromise(instance.ckPost("/changes/zone", { zones: [] }));
    expect(fetchMock).toHaveBeenCalledTimes(2);
    for (const [, init] of fetchMock.mock.calls) {
      expect(init.headers).toMatchObject({
        "User-Agent": "python-requests/2.31.0",
        Referer: "https://www.icloud.com/",
      });
    }
  });
  it("rejects a non-Apple CloudKit host before sending a request", async () => {
    const { instance, fetchMock } = client(() => new Response("{}"), {
      ...saved,
      ckBaseUrl:
        "https://attacker.example/database/1/com.apple.reminders/production/private",
    });
    const result = await Effect.runPromise(
      instance.ckPost("/changes/zone", { zones: [] }).pipe(Effect.result),
    );
    expect(Result.isFailure(result)).toBe(true);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refuses a redirect without forwarding session cookies", async () => {
    const { instance, fetchMock } = client(
      () =>
        new Response(null, {
          status: 302,
          headers: { location: "https://attacker.example/collect" },
        }),
    );
    const result = await Effect.runPromise(
      instance.ckPost("/changes/zone", { zones: [] }).pipe(Effect.result),
    );
    expect(Result.isFailure(result)).toBe(true);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]?.[1]?.redirect).toBe("manual");
  });

  it("retries CloudKit once after refreshing the token session", async () => {
    const { instance, fetchMock, writes } = client((url) => {
      if (url.pathname.endsWith("/accountLogin")) {
        return Response.json({
          dsInfo: { hsaVersion: 2 },
          webservices: {
            ckdatabasews: { url: "https://ckdatabasews.icloud.com" },
          },
        });
      }
      return new Response(JSON.stringify({ zones: [{ records: [] }] }), {
        status: fetchMock.mock.calls.length === 1 ? 401 : 200,
      });
    });
    const result = await Effect.runPromise(
      instance.ckPost("/changes/zone", { zones: [] }),
    );
    expect(result).toEqual({ zones: [{ records: [] }] });
    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(writes.length).toBeGreaterThan(0);
  });

  it("never retries a rejected modify request", async () => {
    const { instance, fetchMock } = client(() =>
      Response.json({ error: { errorCode: "AUTHENTICATION_FAILED" } }, { status: 401 }),
    );
    const result = await Effect.runPromise(
      instance.ckPost("/records/modify", { operations: [] }).pipe(Effect.result),
    );
    expect(Result.isFailure(result)).toBe(true);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("classifies a rate-limited session check without starting sign-in", async () => {
    const { instance, fetchMock } = client(() => Response.json({}, { status: 503 }));
    const result = await Effect.runPromise(instance.verify().pipe(Effect.result));
    expect(Result.isFailure(result)).toBe(true);
    if (Result.isFailure(result)) {
      expect(result.failure.kind).toBe("rate-limited");
    }
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("treats Apple's 421 as an expired session without starting sign-in", async () => {
    const { instance, fetchMock } = client(() => Response.json({}, { status: 421 }));
    expect(await Effect.runPromise(instance.verify())).toBe(false);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]?.[0].pathname).toBe("/setup/ws/1/validate");
  });

  it("allows an explicit sign-in after saved-session validation returns 421", async () => {
    const { instance, fetchMock } = client((url) =>
      Response.json({}, { status: url.pathname.endsWith("/validate") ? 421 : 429 }),
    );
    const result = await Effect.runPromise(instance.begin().pipe(Effect.result));
    expect(Result.isFailure(result) && result.failure.status).toBe(429);
    expect(fetchMock.mock.calls.map(([url]) => url.pathname)).toEqual([
      "/setup/ws/1/validate",
      "/appleauth/auth/signin/init",
    ]);
  });

  it.each([
    [200, true, 200],
    [405, true, 200],
    [405, false, 200],
    [200, true, 405],
    [401, true, 200],
    [429, true, 200],
    [500, true, 200],
    [503, true, 200],
  ] as const)(
    "handles push %s with challenge %s and options %s",
    async (pushStatus, hasChallenge, optionsStatus) => {
      const { instance, fetchMock } = client(
        (url, init) => {
          if (url.pathname.endsWith("/verify/trusteddevice/securitycode")) {
            if (init.method === "POST") return Response.json({}, { status: 400 });
            return Response.json(
              {},
              {
                status: pushStatus,
                headers: hasChallenge
                  ? { scnt: "push-scnt", "x-apple-id-session-id": "push-id" }
                  : {},
              },
            );
          }
          if (url.pathname.endsWith("/signin/init")) {
            return Response.json(
              {
                protocol: "s2k",
                iteration: 1,
                salt: Buffer.alloc(16, 1).toString("base64"),
                b: Buffer.from([2]).toString("base64"),
                c: "opaque-challenge",
              },
              {
                headers: hasChallenge
                  ? { scnt: "fresh-scnt", "x-apple-id-session-id": "fresh-id" }
                  : {},
              },
            );
          }
          if (url.pathname.endsWith("/signin/complete")) {
            return Response.json(
              {},
              {
                status: 409,
                headers: { "x-apple-session-token": "new-session" },
              },
            );
          }
          if (url.pathname.endsWith("/accountLogin")) {
            return Response.json({
              dsInfo: { hsaVersion: 2 },
              hsaTrustedBrowser: false,
            });
          }
          if (url.pathname === "/appleauth/auth")
            return Response.json(
              {},
              {
                status: optionsStatus,
                headers: hasChallenge
                  ? { scnt: "options-scnt", "x-apple-id-session-id": "options-id" }
                  : {},
              },
            );
          return Response.json({});
        },
        { clientId: "auth-test" },
      );
      const result = await Effect.runPromise(instance.begin().pipe(Effect.result));
      if (
        optionsStatus === 200 &&
        (pushStatus === 200 || (pushStatus === 405 && hasChallenge))
      ) {
        expect(Result.isSuccess(result) && result.success).toBe("mfa-required");
      } else {
        expect(Result.isFailure(result) && result.failure.status).toBe(
          optionsStatus !== 200 ? optionsStatus : pushStatus,
        );
        expect(Result.isFailure(result) && result.failure.operation).toBe(
          optionsStatus !== 200 ? "MFA options" : "MFA push",
        );
      }
      const complete = fetchMock.mock.calls.find(([url]) =>
        url.pathname.endsWith("/signin/complete"),
      );
      if (hasChallenge)
        expect(complete?.[1]?.headers).toMatchObject({
          scnt: "fresh-scnt",
          "X-Apple-ID-Session-Id": "fresh-id",
        });
      const pushes = fetchMock.mock.calls.filter(([url]) =>
        url.pathname.endsWith("/verify/trusteddevice/securitycode"),
      );
      expect(pushes).toHaveLength(optionsStatus === 200 ? 1 : 0);
      if (optionsStatus === 200) {
        expect(pushes[0]?.[1]).toMatchObject({ method: "PUT" });
        expect(pushes[0]?.[1].body).toBeUndefined();
        if (hasChallenge)
          expect(pushes[0]?.[1].headers).toMatchObject({
            scnt: "options-scnt",
            "X-Apple-ID-Session-Id": "options-id",
          });
      }
      if (pushStatus === 200 && optionsStatus === 200) {
        const submitted = await Effect.runPromise(
          instance.submit2fa("123456").pipe(Effect.result),
        );
        expect(Result.isFailure(submitted) && submitted.failure.status).toBe(400);
        const last = fetchMock.mock.calls.at(-1)!;
        expect(last[0].pathname).toBe(
          "/appleauth/auth/verify/trusteddevice/securitycode",
        );
        expect(last[1]).toMatchObject({
          method: "POST",
          headers: { scnt: "push-scnt", "X-Apple-ID-Session-Id": "push-id" },
          body: JSON.stringify({ securityCode: { code: "123456" } }),
        });
        expect(
          fetchMock.mock.calls.filter(([url]) => url.pathname.endsWith("/signin/init")),
        ).toHaveLength(1);
      }
      for (const [url, init] of fetchMock.mock.calls) {
        if (url.hostname !== "idmsa.apple.com") continue;
        expect(init.headers).toMatchObject({
          "User-Agent":
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36",
          Referer: url.pathname.includes("/signin/")
            ? "https://www.icloud.com/"
            : "https://idmsa.apple.com",
        });
      }
    },
  );

  it.each([
    [409, 401, true],
    [409, 403, true],
    [409, 421, true],
    [200, 401, false],
    [200, 403, false],
    [200, 421, false],
    [409, 451, false],
    [409, 429, false],
    [409, 500, false],
    [409, 503, false],
    [409, 200, false],
  ] as const)(
    "limits account-login MFA fallback after SRP %s and setup %s",
    async (completeStatus, setupStatus, expectsMfa) => {
      const { instance, fetchMock } = client(
        (url) => {
          if (url.pathname.endsWith("/signin/init")) {
            return Response.json({
              protocol: "s2k",
              iteration: 1,
              salt: Buffer.alloc(16, 1).toString("base64"),
              b: Buffer.from([2]).toString("base64"),
              c: "opaque-challenge",
            });
          }
          if (url.pathname.endsWith("/signin/complete")) {
            return Response.json(
              {},
              {
                status: completeStatus,
                headers: { "x-apple-session-token": "new-session" },
              },
            );
          }
          if (url.pathname.endsWith("/accountLogin")) {
            return Response.json(
              setupStatus === 200 ? { termsUpdateNeeded: true } : {},
              { status: setupStatus },
            );
          }
          return Response.json({});
        },
        { clientId: "auth-test" },
      );
      const result = await Effect.runPromise(instance.begin().pipe(Effect.result));
      if (expectsMfa) {
        expect(Result.isSuccess(result) && result.success).toBe("mfa-required");
      } else {
        expect(Result.isFailure(result) && result.failure.status).toBe(
          setupStatus === 200 ? 451 : setupStatus,
        );
      }
      expect(
        fetchMock.mock.calls.some(([url]) => url.pathname === "/appleauth/auth"),
      ).toBe(expectsMfa);
      expect(
        fetchMock.mock.calls.some(([url]) =>
          url.pathname.endsWith("/verify/trusteddevice/securitycode"),
        ),
      ).toBe(expectsMfa);
    },
  );

  it("never treats code-verification 405 as successful authentication", async () => {
    const { instance, fetchMock } = client(() => Response.json({}, { status: 405 }), {
      clientId: "auth-test",
      scnt: "challenge",
      sessionId: "challenge-id",
    });
    const result = await Effect.runPromise(
      instance.submit2fa("123456").pipe(Effect.result),
    );
    expect(Result.isFailure(result) && result.failure.operation).toBe("MFA verify");
    expect(Result.isFailure(result) && result.failure.status).toBe(405);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]?.[0].pathname).toBe(
      "/appleauth/auth/verify/trusteddevice/securitycode",
    );
  });

  it("drops a stale persisted challenge before retrying SRP init", async () => {
    let initCount = 0;
    const { instance, fetchMock } = client(
      (url) => {
        if (url.pathname.endsWith("/signin/init")) {
          initCount++;
          if (initCount === 1) return Response.json({}, { status: 409 });
          return Response.json({
            protocol: "s2k",
            iteration: 1,
            salt: Buffer.alloc(16, 1).toString("base64"),
            b: Buffer.from([2]).toString("base64"),
            c: "opaque-challenge",
          });
        }
        if (url.pathname.endsWith("/signin/complete")) {
          return Response.json(
            {},
            { status: 200, headers: { "x-apple-session-token": "new-session" } },
          );
        }
        if (url.pathname.endsWith("/accountLogin")) {
          return Response.json({
            dsInfo: { hsaVersion: 2 },
            hsaTrustedBrowser: true,
            webservices: { ckdatabasews: { url: "https://ckdatabasews.icloud.com" } },
          });
        }
        return Response.json({});
      },
      { clientId: "auth-test", scnt: "old-scnt", sessionId: "old-id" },
    );
    expect(await Effect.runPromise(instance.begin())).toBe("ready");
    const inits = fetchMock.mock.calls.filter(([url]) =>
      url.pathname.endsWith("/signin/init"),
    );
    expect(inits).toHaveLength(2);
    expect(inits[0]?.[1]?.headers).toMatchObject({ scnt: "old-scnt" });
    expect(inits[1]?.[1]?.headers).not.toHaveProperty("scnt");
  });

  it("rejects an MFA code without trusting the browser", async () => {
    const { instance, fetchMock } = client(
      () => Response.json({ serviceErrors: [{ code: -21669 }] }, { status: 401 }),
      { ...saved, scnt: "scnt", sessionId: "id" },
    );
    const result = await Effect.runPromise(
      instance.submit2fa("123456").pipe(Effect.result),
    );
    expect(Result.isFailure(result)).toBe(true);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("reports terms without accepting them or exposing response contents", async () => {
    const { instance, fetchMock } = client(() =>
      Response.json({ termsUpdateNeeded: true, secret: "never-log-this" }),
    );
    const result = await Effect.runPromise(instance.verify().pipe(Effect.result));
    expect(Result.isFailure(result)).toBe(true);
    if (Result.isFailure(result)) {
      expect(result.failure.kind).toBe("terms-required");
      expect(JSON.stringify(result.failure)).not.toContain("never-log-this");
    }
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});
