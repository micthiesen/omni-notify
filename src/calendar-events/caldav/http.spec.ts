import { afterEach, describe, expect, it, vi } from "vitest";
import { assertTrustedCaldavUrl, CALDAV_REQUEST_TIMEOUT_MS, propfind } from "./http.js";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("CalDAV HTTP safety", () => {
  it("follows iCloud shard redirects with authorization and a timeout", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(null, {
          status: 302,
          headers: { location: "https://p42-caldav.icloud.com/123/" },
        }),
      )
      .mockResolvedValueOnce(new Response("<multistatus/>", { status: 207 }));
    vi.stubGlobal("fetch", fetchMock);

    const result = await propfind(
      "https://caldav.icloud.com/",
      "Basic secret",
      "0",
      "<propfind/>",
    );

    expect(result.url).toBe("https://p42-caldav.icloud.com/123/");
    expect(fetchMock).toHaveBeenCalledTimes(2);
    for (const [, init] of fetchMock.mock.calls) {
      expect(init.headers.Authorization).toBe("Basic secret");
      expect(init.signal).toBeInstanceOf(AbortSignal);
    }
    expect(CALDAV_REQUEST_TIMEOUT_MS).toBe(15_000);
  });

  it("refuses to forward authorization to an untrusted redirect host", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(null, {
        status: 302,
        headers: { location: "https://attacker.example/collect" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      propfind("https://caldav.icloud.com/", "Basic secret", "0", "<propfind/>"),
    ).rejects.toThrow("Refusing to forward CalDAV credentials");
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("accepts only provider-owned HTTPS collection URLs", () => {
    expect(
      assertTrustedCaldavUrl(
        "https://p03-caldav.icloud.com/123/calendars/home/",
        "icloud",
      ),
    ).toContain("p03-caldav.icloud.com");
    expect(() => assertTrustedCaldavUrl("http://caldav.icloud.com/", "icloud")).toThrow(
      "Untrusted icloud CalDAV URL",
    );
    expect(() =>
      assertTrustedCaldavUrl("https://icloud.com.attacker.example/", "icloud"),
    ).toThrow("Untrusted icloud CalDAV URL");
    expect(() =>
      assertTrustedCaldavUrl("https://calendar.example/", "fastmail"),
    ).toThrow("Untrusted fastmail CalDAV URL");
  });
});
