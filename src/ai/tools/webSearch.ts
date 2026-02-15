import { tool } from "ai";
import got from "got";
import { z } from "zod";
import config from "../../utils/config.js";

interface TavilySearchResponse {
  results: Array<{ title: string; url: string; content: string }>;
  response_time: number;
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
    const { results, response_time } = await got
      .post("https://api.tavily.com/search", {
        json: { query, topic, time_range, max_results: 5 },
        headers: { Authorization: `Bearer ${config.TAVILY_API_KEY}` },
      })
      .json<TavilySearchResponse>();

    return {
      results: results.map(({ title, url, content }) => ({
        title,
        url,
        content,
      })),
      responseTime: response_time,
    };
  },
});
