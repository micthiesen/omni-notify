import { google } from "@ai-sdk/google";
import { webSearch } from "@exalabs/ai-sdk";
import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { generateText, stepCountIs, tool } from "ai";
import { z } from "zod";
import { ScheduledTask } from "../scheduling/ScheduledTask.js";
import config from "../utils/config.js";
import { addBriefingNotification } from "./persistence.js";
import { resolveAllPlaceholders } from "./placeholders.js";

export interface BriefingConfig {
  name: string;
  schedule: string;
  prompt: string;
}

export class BriefingAgentTask extends ScheduledTask {
  public readonly name: string;
  public readonly schedule: string;
  protected readonly prompt: string;

  protected logger: Logger;

  public static create(
    briefingConfig: BriefingConfig,
    parentLogger: Logger,
  ): BriefingAgentTask | null {
    const missing: string[] = [];
    if (!config.GOOGLE_GENERATIVE_AI_API_KEY)
      missing.push("GOOGLE_GENERATIVE_AI_API_KEY");
    if (!config.EXA_API_KEY) missing.push("EXA_API_KEY");

    if (missing.length > 0) {
      parentLogger.info(
        `${briefingConfig.name} disabled: missing ${missing.join(", ")}`,
      );
      return null;
    }

    return new BriefingAgentTask(briefingConfig, parentLogger);
  }

  protected constructor(briefingConfig: BriefingConfig, parentLogger: Logger) {
    super();
    this.name = briefingConfig.name;
    this.schedule = briefingConfig.schedule;
    this.prompt = briefingConfig.prompt;
    this.logger = parentLogger.extend(`${briefingConfig.name}Task`);
  }

  public async run(): Promise<void> {
    const resolvedPrompt = resolveAllPlaceholders(this.prompt, this.name);
    this.logger.info(`Starting briefing agent with prompt:\n${resolvedPrompt}`);

    const tools = {
      web_search: webSearch(),
      send_notification: tool({
        description:
          "Send a push notification to the user with your briefing. Call this once you have something interesting to share.",
        inputSchema: z.object({
          title: z.string().describe("Short title for the notification"),
          message: z.string().describe("The notification body with your summary"),
          url: z.string().url().describe("URL to the source"),
          url_title: z.string().describe("Link text for the URL (e.g. 'Read more')"),
        }),
        execute: async ({ title, message, url, url_title }) => {
          this.logger.info(`Sending notification: ${title}`);
          await notify({ title, message, url, url_title });
          addBriefingNotification(this.name, {
            title,
            message,
            url,
            timestamp: Date.now(),
          });
          return { success: true };
        },
      }),
    };

    const { steps } = await generateText({
      model: google("gemini-3-flash-preview"),
      tools,
      stopWhen: stepCountIs(10),
      onStepFinish: ({ text, toolCalls, toolResults }) => {
        if (text) this.logger.debug(`Step text: ${text}`);
        for (const call of toolCalls) {
          if (call.toolName === "web_search") {
            const input = call.input as { query?: string };
            this.logger.info(`Search: "${input.query}"`);
          } else if (call.toolName !== "send_notification") {
            this.logger.info(`Tool call: ${call.toolName}`, call.input);
          }
          // send_notification already logged in execute callback
        }
        for (const result of toolResults) {
          if (result.toolName === "web_search") {
            const output = result.output as Record<string, unknown>;
            const count = Array.isArray(output?.results) ? output.results.length : "?";
            const time =
              typeof output?.searchTime === "number"
                ? ` (${(output.searchTime / 1000).toFixed(1)}s)`
                : "";
            this.logger.debug(`Search returned ${count} results${time}`);
          } else if (result.toolName !== "send_notification") {
            this.logger.info(`Tool result: ${result.toolName}`, result.output);
          }
        }
      },
      prompt: resolvedPrompt,
    });

    this.logger.info(`Agent completed in ${steps.length} steps`);
  }
}
