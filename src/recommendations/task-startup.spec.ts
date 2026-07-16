import type { Logger } from "@micthiesen/mitools/logging";
import { describe, expect, it, vi } from "vitest";

vi.mock("../utils/config.js", () => ({
  default: {
    RECS_SCHEDULE: "0 0 17 * * 1,3,5",
    TASTE_REFLECTION_SCHEDULE: "0 0 4 * * 0",
    TMDB_API_KEY: "tmdb",
    TAVILY_API_KEY: "tavily",
    OPENAI_API_KEY: "openai",
    PLEX_URL: "http://plex.test",
    PLEX_TOKEN: "plex",
    RADARR_URL: "http://radarr.test",
    RADARR_API_KEY: "radarr",
    RADARR_ROOT_FOLDER_PATH: "/movies",
    RADARR_QUALITY_PROFILE_ID: 1,
    SONARR_URL: "http://sonarr.test",
    SONARR_API_KEY: "sonarr",
    SONARR_ROOT_FOLDER_PATH: "/series",
    SONARR_QUALITY_PROFILE_ID: 1,
  },
}));

import { RecommendationTask } from "./task.js";
import { TasteReflectionTask } from "./taste/task.js";

const logger = {
  extend: vi.fn(),
  info: vi.fn(),
} as unknown as Logger;
(logger.extend as ReturnType<typeof vi.fn>).mockReturnValue(logger);

describe("recommendation task startup policy", () => {
  it("does not run LLM-backed tasks on service startup", () => {
    const recommendations = RecommendationTask.create(logger);
    const tasteReflection = TasteReflectionTask.create(logger);

    expect(recommendations?.runOnStartup).toBe(false);
    expect(tasteReflection?.runOnStartup).toBe(false);
  });
});
