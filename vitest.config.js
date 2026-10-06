import { defineConfig, mergeConfig } from "vitest/config";
import { baseVitestConfig } from "@micthiesen/mitools/vitest";

// Agent worktrees under .claude/ hold other checkouts of this repository.
export default mergeConfig(
  baseVitestConfig,
  defineConfig({ test: { exclude: ["**/.claude/**"] } }),
);
