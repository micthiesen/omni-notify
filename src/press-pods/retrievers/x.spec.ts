import { afterEach, describe, expect, it, vi } from "vitest";
import {
  articleBlocksToMarkdown,
  parseXStatusUrl,
  retrieveArticleX,
  threadTitle,
} from "./x.js";

const ROOT_ID = "2079904005652893709";
const AUTHOR = { id: "author-1", name: "Dmitry Rybin", screen_name: "DmitryRybin1" };
const ROOT_TEXT = `Dinitz-Garg-Goemans conjecture is false. This graph theory problem was open for ~30 years.

The graph below has fractional flow cost 58. Any unsplittable flow (with capacity violation <=15) has cost at least 60.

Chat with GPT 5.6 Pro where this was found: https://chatgpt.com/share/6a60b2eb-0b64-83ee-9c76-7931ca1de063`;
const SECOND_TEXT = `I know counterexamples to old conjectures are becoming a meme at this point. But I really cared about this problem and spent many weeks thinking about it a while ago (in both directions, proof and disproof).

I think almost all graph flows experts thought about this problem.`;
const THIRD_TEXT = `The conjecture was based on absolutely stunning result of Dinitz, Garg, and Goemans: any fractional flow can be routed to unsplittable flow by violating graph capacities by at most max(demand).

The chat with gpt pro here is an absolute meme`;

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("retrieveArticleX", () => {
  it("extracts a complete X Article with headings, footer, and metadata", async () => {
    const articleStatus = {
      id: "2077031491045929255",
      type: "status",
      text: "",
      created_at: "Tue Jul 14 14:04:31 +0000 2026",
      author: { id: "author-ed", name: "Ed Elson", screen_name: "edels0n" },
      media: { photos: [{ url: "https://pbs.twimg.com/media/fallback.jpg" }] },
      article: {
        title: "The Analysts Are Compromised",
        created_at: "2026-07-14T14:04:31.000Z",
        cover_media: {
          media_info: { original_img_url: "https://pbs.twimg.com/media/cover.jpg" },
        },
        content: {
          blocks: [
            { type: "unstyled", text: "The real reason Wall Street loves SpaceX" },
            {
              type: "unstyled",
              text: "Twenty-three years ago, a scandal emerged on Wall Street. Henry Blodget turned out to be privately bearish.",
            },
            { type: "atomic", text: " " },
            { type: "header-two", text: "Déjà Vu" },
            {
              type: "unstyled",
              text: "According to JPMorgan, SpaceX is worth $2.9 trillion.",
            },
            { type: "divider", text: "ignored" },
            { type: "header-three", text: "Here To Stay" },
            {
              type: "unstyled",
              text: "The solution is simple: Fix the incentives. Don’t hold your breath.",
            },
            { type: "unstyled", text: "See you next week,\n\nEd" },
            {
              type: "unstyled",
              text: "Subscribe to Simply Put by Ed Elson on Substack.\n\nA newsletter about business and tech, from the host of Prof G Markets.\n\nOut every Tuesday.",
            },
          ],
        },
      },
    };
    const fetchMock = vi
      .fn()
      .mockResolvedValue(
        jsonResponse({ code: 200, status: articleStatus, thread: [] }),
      );
    vi.stubGlobal("fetch", fetchMock);

    const result = await retrieveArticleX(
      "https://x.com/edels0n/status/2077031491045929255?s=46&t=tracking",
      "PressPods Test",
    );

    expect(result).toEqual({
      title: "The Analysts Are Compromised",
      text: `The real reason Wall Street loves SpaceX

Twenty-three years ago, a scandal emerged on Wall Street. Henry Blodget turned out to be privately bearish.

## Déjà Vu

According to JPMorgan, SpaceX is worth $2.9 trillion.

### Here To Stay

The solution is simple: Fix the incentives. Don’t hold your breath.

See you next week,

Ed

Subscribe to Simply Put by Ed Elson on Substack.

A newsletter about business and tech, from the host of Prof G Markets.

Out every Tuesday.`,
      author: "Ed Elson",
      domain: "x.com",
      url: "https://x.com/edels0n/status/2077031491045929255",
      publishedAt: new Date("2026-07-14T14:04:31.000Z"),
      leadImageUrl: "https://pbs.twimg.com/media/cover.jpg",
    });
    expect(result.text).not.toContain(result.title);
    expect(fetchMock).toHaveBeenCalledWith(
      "https://api.fxtwitter.com/2/thread/2077031491045929255",
      expect.objectContaining({
        headers: { "User-Agent": "PressPods Test", Accept: "application/json" },
        signal: expect.any(AbortSignal),
      }),
    );
  });

  it("assembles only the root author's unique, non-tombstone thread posts", async () => {
    const root = {
      id: ROOT_ID,
      type: "status",
      text: ROOT_TEXT,
      created_timestamp: 1_789_293_021,
      author: AUTHOR,
      media: {
        photos: [
          {
            url: "https://pbs.twimg.com/media/graph.jpg?name=orig",
            altText: "A graph showing the flow counterexample",
          },
        ],
      },
    };
    const fetchMock = vi.fn().mockResolvedValue(
      jsonResponse({
        code: 200,
        status: root,
        thread: [
          root,
          { id: "reply", text: "Unrelated reply", author: { id: "someone-else" } },
          { type: "tombstone", text: "This post was deleted" },
          { id: "second", text: SECOND_TEXT, author: AUTHOR, media: {} },
          root,
          { id: "third", text: THIRD_TEXT, author: AUTHOR, media: {} },
        ],
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const result = await retrieveArticleX(
      `https://x.com/DmitryRybin1/status/${ROOT_ID}`,
      "PressPods Test",
    );

    expect(result.text).toBe(
      `${ROOT_TEXT}\n\nImage description: A graph showing the flow counterexample\n\n${SECOND_TEXT}\n\n${THIRD_TEXT}`,
    );
    expect(result).toMatchObject({
      title:
        "Dinitz-Garg-Goemans conjecture is false. This graph theory problem was open for ~30 years.",
      author: "Dmitry Rybin",
      domain: "x.com",
      url: `https://x.com/DmitryRybin1/status/${ROOT_ID}`,
      publishedAt: new Date(1_789_293_021_000),
      leadImageUrl: "https://pbs.twimg.com/media/graph.jpg?name=orig",
    });
    expect(result.text).not.toContain("Unrelated reply");
    expect(result.text).not.toContain("This post was deleted");
  });

  it("uses the status alone when the API has no thread", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        jsonResponse({
          code: 200,
          status: { id: ROOT_ID, text: ROOT_TEXT, author: AUTHOR, media: {} },
        }),
      ),
    );

    const result = await retrieveArticleX(
      `https://twitter.com/DmitryRybin1/status/${ROOT_ID}`,
      "test",
    );
    expect(result.text).toBe(ROOT_TEXT);
    expect(result.url).toBe(`https://x.com/DmitryRybin1/status/${ROOT_ID}`);
  });

  it("falls back to connected thread text for an empty X Article body", async () => {
    const root = {
      id: ROOT_ID,
      text: "",
      author: AUTHOR,
      article: {
        title: "Media-only article",
        content: { blocks: [{ type: "atomic", text: " " }] },
      },
    };
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        jsonResponse({
          code: 200,
          status: root,
          thread: [root, { id: "continuation", text: SECOND_TEXT, author: AUTHOR }],
        }),
      ),
    );

    const result = await retrieveArticleX(
      `https://x.com/i/web/status/${ROOT_ID}`,
      "test",
    );
    expect(result.text).toBe(SECOND_TEXT);
    expect(result.title).toBe("Media-only article");
  });

  it("reports URL, HTTP, and API errors clearly", async () => {
    expect(() => parseXStatusUrl("https://x.com/DmitryRybin1")).toThrow(
      "not a status permalink",
    );
    expect(() => parseXStatusUrl("https://example.com/user/status/123")).toThrow(
      "Unsupported X URL hostname",
    );

    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response("down", { status: 503 })),
    );
    await expect(
      retrieveArticleX("https://x.com/user/status/123", "test"),
    ).rejects.toThrow("HTTP 503");

    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValue(jsonResponse({ code: 404, message: "Tweet not found" })),
    );
    await expect(
      retrieveArticleX("https://x.com/user/status/123", "test"),
    ).rejects.toThrow("code 404: Tweet not found");
  });

  it("accepts mobile and media permalink variants", () => {
    expect(
      parseXStatusUrl(`https://mobile.twitter.com/user/status/${ROOT_ID}/photo/1`),
    ).toEqual({
      id: ROOT_ID,
      screenName: "user",
      canonicalUrl: `https://x.com/user/status/${ROOT_ID}`,
    });
  });
});

describe("X parsing helpers", () => {
  it("does not split a word when shortening a thread title", () => {
    const title = threadTitle("word ".repeat(40), 40);
    expect(title).toBe("word word word word word word word…");
  });

  it("preserves only prose and supported headings from article blocks", () => {
    expect(
      articleBlocksToMarkdown([
        { type: "header-two", text: "Section" },
        { type: "atomic", text: "media placeholder" },
        { type: "unstyled", text: "Body" },
      ]),
    ).toBe("## Section\n\nImage description: media placeholder\n\nBody");
  });
});
