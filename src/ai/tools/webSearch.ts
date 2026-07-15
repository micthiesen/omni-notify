import { tool } from "ai";
import got from "got";
import { z } from "zod";
import config from "../../utils/config.js";

interface TavilySearchResponse {
  results: Array<{ title: string; url: string; content: string }>;
  response_time: number;
}

export interface WebSearchResult {
  title: string;
  url: string;
  content: string;
}

export async function searchWeb(options: {
  query: string;
  topic?: "general" | "news";
  timeRange?: "day" | "week" | "month" | "year";
  maxResults?: number;
  maxContentChars?: number;
}): Promise<{ results: WebSearchResult[]; responseTime: number }> {
  const { results, response_time } = await got
    .post("https://api.tavily.com/search", {
      json: {
        query: options.query,
        topic: options.topic,
        time_range: options.timeRange,
        max_results: options.maxResults ?? 5,
      },
      headers: { Authorization: `Bearer ${config.TAVILY_API_KEY}` },
    })
    .json<TavilySearchResponse>();

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
}

export const webSearch = tool({
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
  execute: async ({ query, topic, time_range }) => {
    return searchWeb({ query, topic, timeRange: time_range });
  },
});
