import { readdirSync, readFileSync } from "node:fs";
import { basename, join } from "node:path";
import type { Logger } from "@micthiesen/mitools/logging";
import matter from "gray-matter";
import { validate } from "node-cron";
import { z } from "zod";
import config from "../utils/config.js";
import type { BriefingConfig } from "./BriefingAgentTask.js";

const frontmatterSchema = z.object({
  schedule: z.string(),
});

export function loadBriefingConfigs(logger: Logger): BriefingConfig[] {
  const briefingsPath = config.BRIEFINGS_PATH;
  if (!briefingsPath) {
    logger.info("No BRIEFINGS_PATH configured, skipping briefing tasks");
    return [];
  }

  let files: string[];
  try {
    files = readdirSync(briefingsPath).filter((f) => f.endsWith(".md"));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      logger.warn(`Briefings folder not found: ${briefingsPath}`);
      return [];
    }
    throw error;
  }

  const configs: BriefingConfig[] = [];

  for (const file of files) {
    const filePath = join(briefingsPath, file);
    const raw = readFileSync(filePath, "utf-8");
    const { data, content } = matter(raw);

    const name = basename(file, ".md");
    const parsed = frontmatterSchema.safeParse(data);
    if (!parsed.success) {
      logger.warn(`Skipping ${file}: missing or invalid 'schedule' field`);
      continue;
    }

    const { schedule } = parsed.data;
    if (!validate(schedule)) {
      logger.warn(`Skipping ${file}: invalid cron expression "${schedule}"`);
      continue;
    }

    const prompt = content.trim();
    if (!prompt) {
      logger.warn(`Skipping ${file}: empty body`);
      continue;
    }

    configs.push({ name, schedule, prompt });
  }

  logger.info(`Loaded ${configs.length} briefing config(s) from ${briefingsPath}`);
  return configs;
}
