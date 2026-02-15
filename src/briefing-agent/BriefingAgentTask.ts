import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { generateText, stepCountIs, tool } from "ai";
import { z } from "zod";
import { getBriefingModel } from "../ai/registry.js";
import { fetchUrl } from "../ai/tools/fetchUrl.js";
import { webSearch } from "../ai/tools/webSearch.js";
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
    if (!config.TAVILY_API_KEY) {
      parentLogger.info(`${briefingConfig.name} disabled: missing TAVILY_API_KEY`);
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
    const { model, modelId } = getBriefingModel();
    this.logger.info(
      `Starting briefing agent (${modelId}) with prompt:\n${resolvedPrompt}`,
    );

    const tools = {
      web_search: webSearch,
      fetch_url: fetchUrl,
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
          await notify({
            title,
            message,
            url,
            url_title,
            token: config.PUSHOVER_BRIEFING_TOKEN,
          });
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
      model,
      providerOptions: {
        google: { thinkingConfig: { thinkingLevel: "high" as const } },
      },
      tools,
      stopWhen: stepCountIs(10),
      onStepFinish: ({ text, toolCalls, toolResults }) => {
        if (text) this.logger.debug(`Step text: ${text}`);
        for (const call of toolCalls) {
          if (call.toolName === "web_search") {
            const input = call.input as { query?: string };
            this.logger.info(`Search: "${input.query}"`);
          } else if (call.toolName === "fetch_url") {
            const input = call.input as { url?: string };
            this.logger.info(`Fetching: ${input.url}`);
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
              typeof output?.responseTime === "number"
                ? ` (${output.responseTime.toFixed(1)}s)`
                : "";
            this.logger.debug(`Search returned ${count} results${time}`);
          } else if (result.toolName === "fetch_url") {
            const output = result.output as Record<string, unknown>;
            const chars =
              typeof output?.content === "string" ? output.content.length : "?";
            const truncated = output?.truncated ? " (truncated)" : "";
            this.logger.debug(`Fetched ${chars} chars${truncated}`);
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
