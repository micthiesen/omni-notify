import { Effect } from "effect";
import { describe, expect, it, vi } from "vitest";
import { createExecutorEventAuthorizer, executorOwnerId } from "./executorAuth.js";

describe("Executor event owner revalidation", () => {
  it("distinguishes temporary HTTP failures from revoked access", async () => {
    const owner = executorOwnerId("user", "client");
    for (const status of [429, 500, 503]) {
      const authorize = createExecutorEventAuthorizer(
        "http://executor:4788/api/auth/mcp/get-session",
        async () => new Response("unavailable", { status }),
      );
      const result = await Effect.runPromise(
        authorize(owner, "Bearer opaque").pipe(Effect.result),
      );
      expect(result._tag).toBe("Failure");
    }
    const revoked = createExecutorEventAuthorizer(
      "http://executor:4788/api/auth/mcp/get-session",
      async () => new Response(null, { status: 401 }),
    );
    expect(await Effect.runPromise(revoked(owner, "Bearer opaque"))).toBeNull();
  });
  it("requires the exact user/client pair and a live access token", async () => {
    const expiresAt = new Date(Date.now() + 60_000).toISOString();
    const request = vi.fn(async () =>
      Response.json({
        userId: "user-1",
        clientId: "client-2",
        accessTokenExpiresAt: expiresAt,
        accessToken: "private-upstream-value",
      }),
    );
    const authorize = createExecutorEventAuthorizer(
      "http://executor:3000/api/auth/mcp/get-session",
      request as typeof fetch,
    );
    expect(
      await Effect.runPromise(
        authorize(executorOwnerId("user-1", "client-2"), "Bearer opaque"),
      ),
    ).toBe(Date.parse(expiresAt));
    expect(
      await Effect.runPromise(
        authorize(executorOwnerId("user-1", "other"), "Bearer opaque"),
      ),
    ).toBeNull();
    expect(request).toHaveBeenCalledWith(
      new URL("http://executor:3000/api/auth/mcp/get-session"),
      expect.objectContaining({
        method: "GET",
        redirect: "error",
        headers: expect.objectContaining({ Authorization: "Bearer opaque" }),
      }),
    );
  });

  it("rejects expired, malformed and missing sessions", async () => {
    const owner = executorOwnerId("user", "client");
    for (const body of [
      null,
      {
        userId: "user",
        clientId: "client",
        accessTokenExpiresAt: "2020-01-01T00:00:00Z",
      },
      { userId: "user", clientId: "client", accessTokenExpiresAt: "not-a-date" },
    ]) {
      const authorize = createExecutorEventAuthorizer(
        "http://executor:3000/api/auth/mcp/get-session",
        async () => Response.json(body),
      );
      expect(await Effect.runPromise(authorize(owner, "Bearer opaque"))).toBeNull();
    }
  });
});
