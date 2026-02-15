import type { Logger } from "@micthiesen/mitools/logging";
import { getFilterModel } from "../../ai/registry.js";
import type { Platform } from "../platforms/index.js";
import { generateStructuredOutput } from "./generateStructuredOutput.js";
import {
  type ChannelFilter,
  type ChannelsConfig,
  type FilterResult,
  filterDecisionSchema,
  type StreamContext,
} from "./types.js";

const SYSTEM_PROMPT = `You are a stream notification filter. Based on the user's preferences and the stream information, decide whether to send a notification.

Respond with:
- shouldNotify: true if this stream matches the user's interests
- reason: a brief explanation

Be conservative: if uncertain, lean toward notifying.`;

type ChannelInfo = { username: string; displayName: string; platform: Platform };

export class StreamFilterService {
  private config: ChannelsConfig;
  private logger: Logger;

  public constructor(config: ChannelsConfig, parentLogger: Logger) {
    this.config = config;
    this.logger = parentLogger.extend("StreamFilterService");
  }

  public logFilterStatus(channels: ChannelInfo[]): void {
    const withFilters: string[] = [];
    const withoutFilters: string[] = [];

    for (const { username, displayName, platform } of channels) {
      const filter = this.getFilter(platform, username);
      const label =
        displayName !== username ? `${displayName} (${username})` : username;

      if (filter) {
        const fallback = filter.defaultOnError ? "notify" : "skip";
        withFilters.push(`${label} [${platform}, fallback: ${fallback}]`);
      } else {
        withoutFilters.push(label);
      }
    }

    if (withFilters.length > 0) {
      this.logger.info(`Channels with filters: ${withFilters.join(", ")}`);
    }
    if (withoutFilters.length > 0) {
      this.logger.info(`Channels without filters: ${withoutFilters.join(", ")}`);
    }
  }

  private getFilter(platform: Platform, username: string): ChannelFilter | null {
    return this.config[platform]?.[username]?.filter ?? null;
  }

  public async shouldNotify(context: StreamContext): Promise<FilterResult> {
    const filter = this.getFilter(context.platform, context.username);

    // No filter configured - always notify
    if (!filter) {
      return { shouldNotify: true, reason: "No filter configured", wasFiltered: false };
    }

    try {
      const userPrompt = this.buildUserPrompt(context, filter.prompt);
      const { model } = getFilterModel();

      const { output, attempts } = await generateStructuredOutput({
        model,
        schema: filterDecisionSchema,
        system: SYSTEM_PROMPT,
        prompt: userPrompt,
        onRetry: (attempt, error) => {
          this.logger.debug(
            `Filter retry ${attempt} for ${context.displayName}: ${error}`,
          );
        },
      });

      if (attempts > 1) {
        this.logger.debug(
          `Filter for ${context.displayName} succeeded after ${attempts} attempts`,
        );
      }

      this.logger.debug(
        `Filter decision for ${context.displayName}: ${output.shouldNotify} - ${output.reason}`,
      );

      return { ...output, wasFiltered: true };
    } catch (error) {
      this.logger.warn(
        `Filter error for ${context.displayName}, using default (${filter.defaultOnError}): ${error}`,
      );
      return {
        shouldNotify: filter.defaultOnError,
        reason: `Filter error, using default: ${filter.defaultOnError}`,
        wasFiltered: false,
      };
    }
  }

  private buildUserPrompt(context: StreamContext, filterPrompt: string): string {
    const parts = [
      `**Stream Information:**`,
      `- Streamer: ${context.displayName}`,
      `- Platform: ${context.platform}`,
      `- Title: ${context.title}`,
    ];

    if (context.category) {
      parts.push(`- Category: ${context.category}`);
    }

    parts.push("", `**User Preferences:**`, filterPrompt);

    return parts.join("\n");
  }
}
