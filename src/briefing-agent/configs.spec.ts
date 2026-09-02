import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Effect } from "effect";
import { runTest } from "../live-check/testRuntime.js";

vi.mock("../utils/config.js", () => ({
  default: { BRIEFINGS_PATH: undefined },
}));

import config from "../utils/config.js";
import { loadBriefingConfigs } from "./configs.js";

const mockLogger = {
  info: vi.fn(() => Effect.void),
  warn: vi.fn(() => Effect.void),
  error: vi.fn(() => Effect.void),
  debug: vi.fn(() => Effect.void),
  extend: vi.fn(() => mockLogger),
};

let testDir: string;

beforeEach(() => {
  testDir = join(tmpdir(), `briefings-test-${Date.now()}`);
  mkdirSync(testDir, { recursive: true });
  vi.clearAllMocks();
});

afterEach(() => {
  rmSync(testDir, { recursive: true, force: true });
});

function setBriefingsPath(path: string | undefined) {
  (config as { BRIEFINGS_PATH: string | undefined }).BRIEFINGS_PATH = path;
}

function writeFile(name: string, content: string) {
  writeFileSync(join(testDir, name), content, "utf-8");
}

describe("loadBriefingConfigs", () => {
  it("returns [] when BRIEFINGS_PATH is unset", async () => {
    setBriefingsPath(undefined);
    const result = await runTest(loadBriefingConfigs(mockLogger as never));
    expect(result).toEqual([]);
    expect(mockLogger.info).toHaveBeenCalledWith(
      expect.stringContaining("No BRIEFINGS_PATH"),
    );
  });

  it("returns [] and warns when folder does not exist", async () => {
    setBriefingsPath("/nonexistent/path");
    const result = await runTest(loadBriefingConfigs(mockLogger as never));
    expect(result).toEqual([]);
    expect(mockLogger.warn).toHaveBeenCalledWith(
      expect.stringContaining("Briefings folder not found"),
    );
  });

  it("loads a valid .md file", async () => {
    setBriefingsPath(testDir);
    writeFile(
      "TestBriefing.md",
      `---\nschedule: "0 0 8 * * *"\n---\nYou are a test assistant.`,
    );
    const result = await runTest(loadBriefingConfigs(mockLogger as never));
    expect(result).toEqual([
      {
        name: "TestBriefing",
        schedule: "0 0 8 * * *",
        prompt: "You are a test assistant.",
      },
    ]);
  });

  it("skips files with missing schedule", async () => {
    setBriefingsPath(testDir);
    writeFile("NoSchedule.md", `---\ntitle: "oops"\n---\nSome prompt.`);
    const result = await runTest(loadBriefingConfigs(mockLogger as never));
    expect(result).toEqual([]);
    expect(mockLogger.warn).toHaveBeenCalledWith(
      expect.stringContaining("Skipping NoSchedule.md"),
    );
  });

  it("skips files with invalid cron expression", async () => {
    setBriefingsPath(testDir);
    writeFile("BadCron.md", `---\nschedule: "not a cron"\n---\nSome prompt.`);
    const result = await runTest(loadBriefingConfigs(mockLogger as never));
    expect(result).toEqual([]);
    expect(mockLogger.warn).toHaveBeenCalledWith(
      expect.stringContaining("invalid cron expression"),
    );
  });

  it("skips files with empty body", async () => {
    setBriefingsPath(testDir);
    writeFile("EmptyBody.md", `---\nschedule: "0 0 8 * * *"\n---\n`);
    const result = await runTest(loadBriefingConfigs(mockLogger as never));
    expect(result).toEqual([]);
    expect(mockLogger.warn).toHaveBeenCalledWith(expect.stringContaining("empty body"));
  });

  it("ignores non-.md files", async () => {
    setBriefingsPath(testDir);
    writeFile("readme.txt", `---\nschedule: "0 0 8 * * *"\n---\nPrompt.`);
    writeFile("Valid.md", `---\nschedule: "0 0 8 * * *"\n---\nActual prompt.`);
    const result = await runTest(loadBriefingConfigs(mockLogger as never));
    expect(result).toHaveLength(1);
    expect(result[0].name).toBe("Valid");
  });

  it("loads multiple valid files", async () => {
    setBriefingsPath(testDir);
    writeFile("Alpha.md", `---\nschedule: "0 0 8 * * *"\n---\nPrompt A.`);
    writeFile("Beta.md", `---\nschedule: "0 0 12 * * *"\n---\nPrompt B.`);
    const result = await runTest(loadBriefingConfigs(mockLogger as never));
    expect(result).toHaveLength(2);
    const names = result.map((c) => c.name);
    expect(names).toContain("Alpha");
    expect(names).toContain("Beta");
  });
});
