import type { Logger } from "@micthiesen/mitools/logging";
import PQueue from "p-queue";
import { loadChannelsConfig } from "../filters/index.js";
import { Platform } from "../platforms/index.js";
import config, { type ChannelEntry } from "../utils/config.js";
import LiveCheckTask from "./LiveCheckTask.js";
import type { Task } from "./types.js";

export default class TaskManager {
  private queue: PQueue;
  private tasks: Task[];
  private logger: Logger;

  constructor(parentLogger: Logger) {
    this.logger = parentLogger.extend("TaskManager");
    this.queue = new PQueue({ concurrency: 1 });

    const channels: [Platform, ChannelEntry[]][] = [
      [Platform.YouTube, config.YT_CHANNEL_NAMES],
      [Platform.Twitch, config.TWITCH_CHANNEL_NAMES],
    ];

    this.validateNoDuplicateUsernames(channels);

    const channelsConfig = loadChannelsConfig(this.logger);
    this.tasks = [new LiveCheckTask(channels, channelsConfig, this.logger)];
  }

  private validateNoDuplicateUsernames(channels: [Platform, ChannelEntry[]][]): void {
    const seen = new Map<string, Platform>();

    for (const [platform, entries] of channels) {
      for (const { username } of entries) {
        const existing = seen.get(username);
        if (existing) {
          throw new Error(
            `Duplicate username "${username}" found on ${existing} and ${platform}. ` +
              "Each username must be unique across all platforms due to database key constraints.",
          );
        }
        seen.set(username, platform);
      }
    }
  }

  public async runTasks(): Promise<void> {
    for (const task of this.tasks) {
      this.queue.add(async () => {
        try {
          this.logger.debug(`Running task: ${task.name}`);
          await task.run();
        } catch (err) {
          this.logger.error(`Error running task: ${task.name}`, err);
        }
      });
    }
  }

  public async waitForPending(): Promise<void> {
    if (this.queue.size > 0 || this.queue.pending > 0) {
      this.logger.info(
        `Waiting for ${this.queue.size} queued and ${this.queue.pending} pending tasks...`,
      );
      await this.queue.onIdle();
    }
  }
}
