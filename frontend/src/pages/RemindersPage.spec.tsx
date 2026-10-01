import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Effect } from "effect";
import { afterEach, describe, expect, it, vi } from "vitest";
import RemindersPage, { remindersRequest } from "./RemindersPage";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("Reminders administration page", () => {
  it("keeps controls behind HTTPS", () => {
    vi.stubGlobal("window", { location: { protocol: "http:" } });
    const insecure = renderToStaticMarkup(createElement(RemindersPage));
    expect(insecure).toContain("Open this page over HTTPS");
    expect(insecure).not.toContain("Start sign-in");

    vi.stubGlobal("window", { location: { protocol: "https:" } });
    const secure = renderToStaticMarkup(createElement(RemindersPage));
    expect(secure).toContain("Loading connection status");
    expect(secure).not.toContain('type="password"');
    expect(secure).not.toContain("Start sign-in");
    expect(secure).toContain("Keep Advanced Data Protection enabled");
  });

  it("requests only the fixed same-origin route without credentials", async () => {
    const fetchMock = vi.fn(() =>
      Promise.resolve(
        Response.json({ status: { enabled: true, phase: "authentication-needed" } }),
      ),
    );
    vi.stubGlobal("fetch", fetchMock);
    const result = await Effect.runPromise(remindersRequest("status"));
    expect(result.phase).toBe("authentication-needed");
    const [path, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe("/api/reminders/status");
    expect(init.method).toBe("GET");
    expect(init.credentials).toBe("omit");
    expect(init.cache).toBe("no-store");
    expect(init.redirect).toBe("error");
    expect(init.headers).not.toHaveProperty("Authorization");
  });

  it("rejects unexpected status payloads", async () => {
    vi.stubGlobal("fetch", () =>
      Promise.resolve(
        Response.json({ status: { enabled: true, phase: "admin-secret" } }),
      ),
    );
    await expect(Effect.runPromise(remindersRequest("status"))).rejects.toThrow(
      "Invalid Reminders response",
    );
  });

  it("shows bounded application failure details instead of a generic 502", async () => {
    const status = {
      enabled: true,
      phase: "unsupported-protocol",
      diagnostic: {
        stage: "account-session",
        category: "apple-response",
        httpStatus: 421,
      },
    };
    vi.stubGlobal("fetch", () =>
      Promise.resolve(Response.json({ status }, { status: 502 })),
    );
    expect(await Effect.runPromise(remindersRequest("start"))).toEqual(status);
  });

  it("keeps proxy failures distinct and never renders arbitrary error bodies", async () => {
    vi.stubGlobal("fetch", () =>
      Promise.resolve(new Response("secret upstream HTML", { status: 502 })),
    );
    await expect(Effect.runPromise(remindersRequest("start"))).rejects.toThrow(
      "Reminders request failed (502)",
    );
  });
});
