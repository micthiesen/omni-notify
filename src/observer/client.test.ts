import { Effect } from "effect";
import { describe, expect, it, vi } from "vitest";
import { ObserverClient, ObserverClientError } from "./client.js";

const issue = (id: number) => ({
  id,
  issueType: 1,
  status: 1,
  createdAt: "2026-01-01T00:00:00.000Z",
  updatedAt: "2026-01-01T00:00:00.000Z",
  media: { id: 44, tmdbId: 123, tvdbId: null, status: 5 },
  comments: [{ id: 9, message: "broken", user: null }],
});

describe("ObserverClient", () => {
  it("lists bounded pages and decodes issue media and comments", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
      const page = new URL(String(input)).searchParams.get("skip");
      return Response.json({
        pageInfo: { page: page === "0" ? 1 : 2, pages: 2, results: 2 },
        results: page === "0" ? [issue(1), issue(2)] : [issue(3)],
      });
    });
    const client = new ObserverClient({
      url: "https://observer.example/",
      apiKey: "secret",
      fetchImpl,
    });

    const issues = await Effect.runPromise(
      client.listIssues({ filter: "all", maxRecords: 3 }),
    );

    expect(issues.map(({ id }) => id)).toEqual([1, 2, 3]);
    expect(fetchImpl).toHaveBeenCalledTimes(2);
    const [requestUrl, requestInit] = fetchImpl.mock.calls[0]!;
    expect(String(requestUrl)).toContain("/api/v1/issue?take=3&skip=0&filter=all");
    expect(new Headers(requestInit?.headers).get("X-Api-Key")).toBe("secret");
    expect(issues[0]?.media?.id).toBe(44);
    expect(issues[0]?.comments?.[0]?.message).toBe("broken");
  });

  it("posts comments and resolves issues with the documented endpoints", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
      const path = new URL(String(input)).pathname;
      if (path.endsWith("/comment")) return Response.json(issue(7));
      return Response.json(issue(7));
    });
    const client = new ObserverClient({
      url: "https://observer.example",
      apiKey: "secret",
      fetchImpl,
    });

    await Effect.runPromise(client.addComment(7, "Repair queued"));
    await Effect.runPromise(client.resolveIssue(7));

    expect(fetchImpl).toHaveBeenNthCalledWith(
      1,
      "https://observer.example/api/v1/issue/7/comment",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ message: "Repair queued" }),
      }),
    );
    expect(fetchImpl).toHaveBeenNthCalledWith(
      2,
      "https://observer.example/api/v1/issue/7/resolved",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("returns a typed error without exposing the API key", async () => {
    const fetchImpl = vi.fn(async () => Response.json({ nope: true }));
    const client = new ObserverClient({
      url: "https://observer.example",
      apiKey: "super-secret",
      fetchImpl,
    });

    const result = await Effect.runPromiseExit(client.getIssue(4));

    expect(result._tag).toBe("Failure");
    expect(String(result)).not.toContain("super-secret");
    expect((await Effect.runPromiseExit(client.getIssue(4)))._tag).toBe("Failure");
    expect(
      new ObserverClientError({ operation: "x", cause: new Error("y") }),
    ).toBeInstanceOf(ObserverClientError);
  });
});
