import { beforeEach, describe, expect, it, vi } from "vitest";
import { JINA_READER_CENTS_PER_TOKEN, retrieveArticleJina } from "./jina.js";

const mocks = vi.hoisted(() => ({
  got: vi.fn(),
  json: vi.fn(),
  recordCostEventSafely: vi.fn(),
}));

vi.mock("got", () => ({ default: mocks.got }));
vi.mock("../../utils/config.js", () => ({
  default: { JINA_API_KEY: "test-key" },
}));
vi.mock("../../costs/persistence.js", () => ({
  currentCostFeature: () => "press-pods",
  recordCostEventSafely: mocks.recordCostEventSafely,
}));

const HTML = `<html><head><title>Fallback title</title></head><body><article>${"Article text. ".repeat(20)}</article></body></html>`;

describe("retrieveArticleJina", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.got.mockReturnValue({ json: mocks.json });
  });

  it("requests JSON usage and records an estimated token cost", async () => {
    mocks.json.mockResolvedValue({
      data: {
        title: "Jina title",
        content: HTML,
        usage: { tokens: 1_234 },
      },
    });

    const article = await retrieveArticleJina("https://example.com/story", "ignored");

    expect(article).toMatchObject({
      title: "Jina title",
      domain: "example.com",
      url: "https://example.com/story",
    });
    expect(mocks.got).toHaveBeenCalledWith(
      "https://r.jina.ai/https://example.com/story",
      expect.objectContaining({
        headers: expect.objectContaining({
          Accept: "application/json",
          "X-Respond-With": "html",
        }),
      }),
    );
    expect(mocks.recordCostEventSafely).toHaveBeenCalledWith(
      expect.objectContaining({
        service: "jina",
        model: "reader",
        costCents: 1_234 * JINA_READER_CENTS_PER_TOKEN,
        priceStatus: "estimated",
        usage: { requests: 1, outputTokens: 1_234 },
      }),
    );
  });

  it("keeps missing usage explicitly unpriced instead of inventing a cost", async () => {
    mocks.json.mockResolvedValue({ data: { content: HTML } });

    await retrieveArticleJina("https://example.com/story", "ignored");

    expect(mocks.recordCostEventSafely).toHaveBeenCalledWith(
      expect.objectContaining({
        costCents: null,
        priceStatus: "unknown",
        usage: { requests: 1 },
      }),
    );
  });
});
