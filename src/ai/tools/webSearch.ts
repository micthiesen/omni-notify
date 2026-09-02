import type { EffectRunner } from "@micthiesen/mitools/boundary";
import type { Docstore } from "@micthiesen/mitools/docstore";
import type { Logger } from "@micthiesen/mitools/logging";
import { tool } from "ai";
import { Data, Effect, Schema } from "effect";
import { z } from "zod";
import { currentCostFeature, recordCostEventSafely } from "../../costs/persistence.js";
import {
  fetchPublicText,
  PUBLIC_HTTP_USER_AGENT,
  type PublicTextRequest,
} from "../../effect/publicHttp.js";
import config from "../../utils/config.js";

const TAVILY_SEARCH_URL = "https://api.tavily.com/search";
const TAVILY_RESPONSE_MAX_BYTES = 1024 * 1024;

const TavilySearchResponse = Schema.Struct({
  results: Schema.Array(
    Schema.Struct({
      title: Schema.String,
      url: Schema.String,
      content: Schema.String,
    }),
  ),
  response_time: Schema.Number,
});

export class WebSearchError extends Data.TaggedError("WebSearchError")<{
  readonly cause: unknown;
}> {
  public override get message(): string {
    return `Web search failed: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

export interface WebSearchResult {
  title: string;
  url: string;
  content: string;
}

export function searchWebEffect(
  options: {
    query: string;
    topic?: "general" | "news";
    timeRange?: "day" | "week" | "month" | "year";
    maxResults?: number;
    maxContentChars?: number;
  },
  dependencies: {
    readonly request?: PublicTextRequest;
    readonly maxResponseBytes?: number;
  } = {},
): Effect.Effect<
  { results: WebSearchResult[]; responseTime: number },
  WebSearchError,
  Logger | Docstore
> {
  return Effect.gen(function* () {
    const responseText = yield* fetchPublicText(
      TAVILY_SEARCH_URL,
      {
        method: "POST",
        json: {
          query: options.query,
          topic: options.topic,
          time_range: options.timeRange,
          max_results: options.maxResults ?? 5,
        },
        headers: {
          Authorization: `Bearer ${config.TAVILY_API_KEY}`,
          "User-Agent": PUBLIC_HTTP_USER_AGENT,
        },
        timeout: { request: 15_000 },
      },
      "Tavily search request failed",
      dependencies.request,
      dependencies.maxResponseBytes ?? TAVILY_RESPONSE_MAX_BYTES,
    ).pipe(Effect.mapError((cause) => new WebSearchError({ cause })));
    const { results, response_time } = yield* Schema.decodeUnknownEffect(
      Schema.fromJsonString(TavilySearchResponse),
    )(responseText).pipe(Effect.mapError((cause) => new WebSearchError({ cause })));

    // Default/basic search consumes one credit. Use Tavily's public pay-as-you-go
    // rate as an estimate; subscription/free-plan billing can make actual spend lower.
    yield* recordCostEventSafely({
      category: "search",
      feature: currentCostFeature("web-search"),
      operation: "search",
      service: "tavily",
      model: "basic",
      costCents: 0.8,
      priceStatus: "estimated",
      usage: { requests: 1, credits: 1 },
    });

    return {
      results: results.map(({ title, url, content }) => ({
        title,
        url,
        content:
          options.maxContentChars === undefined
            ? content
            : content.slice(0, options.maxContentChars),
      })),
      responseTime: response_time,
    };
  });
}

export function makeWebSearchTool(
  runner: EffectRunner<Logger | Docstore>,
  dependencies: {
    readonly request?: PublicTextRequest;
    readonly maxResponseBytes?: number;
  } = {},
) {
  return tool({
    description:
      "Search the web for current information. Use topic 'news' for current events and breaking news.",
    inputSchema: z.object({
      query: z.string().describe("The search query"),
      topic: z
        .enum(["general", "news"])
        .optional()
        .describe("'general' for broad searches, 'news' for current events"),
      time_range: z
        .enum(["day", "week", "month", "year"])
        .optional()
        .describe("Filter results by recency"),
    }),
    execute: ({ query, topic, time_range }) =>
      runner.runPromise(
        searchWebEffect({ query, topic, timeRange: time_range }, dependencies),
      ),
  });
}
