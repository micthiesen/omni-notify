import { google } from "@ai-sdk/google";
import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { generateText, stepCountIs, tool } from "ai";
import { z } from "zod";
import { ScheduledTask } from "../scheduling/ScheduledTask.js";

export default class NewsAgentTask extends ScheduledTask {
  public readonly name = "NewsAgent";
  public readonly schedule = "0 0 8 * * *"; // 8am daily (server timezone)

  private logger: Logger;

  public constructor(parentLogger: Logger) {
    super();
    this.logger = parentLogger.extend("NewsAgentTask");
  }

  public async run(): Promise<void> {
    this.logger.info("Starting news agent");

    const tools = {
      google_search: google.tools.googleSearch({}),
      send_notification: tool({
        description:
          "Send a push notification to the user with your news brief. Call this once you have something interesting to share.",
        inputSchema: z.object({
          title: z.string().describe("Short title for the notification"),
          message: z.string().describe("The notification body with your news summary"),
          url: z.string().url().describe("URL to the article"),
          url_title: z.string().describe("Link text for the URL (e.g. 'Read on BBC')"),
        }),
        execute: async ({ title, message, url, url_title }) => {
          this.logger.info(`Sending notification: ${title}`);
          await notify({ title, message, url, url_title });
          return { success: true };
        },
      }),
    };

    const { steps } = await generateText({
      model: google("gemini-3-flash-preview"),
      tools,
      stopWhen: stepCountIs(5),
      prompt: `You are a morning news assistant focused on Canadian news.

Search for the most important Canada-related news from the past 24 hours. Look for:
- Significant developments: policy changes, major events, economic news, scientific breakthroughs
- Objective reporting from reputable sources (CBC, Globe and Mail, Reuters, etc.)
- News that has real-world impact, not opinion pieces or clickbait

Pick the single most important story and send a notification with a concise summary (2-3 sentences) of what happened and why it matters.`,
    });

    this.logger.info(`Agent completed in ${steps.length} steps`);
  }
}
