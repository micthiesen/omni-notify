import fs from "node:fs/promises";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { Effect, Exit } from "effect";
import { runTest } from "../live-check/testRuntime.js";
import type { PressPodsEpisodeData } from "./persistence.js";
import { persistEpisodeWithAudio } from "./pipeline.js";
import { PressPodsError } from "./effect.js";
import { episodeAudioPath } from "./storage.js";

describe("persistEpisodeWithAudio", () => {
  it("removes the final MP3 when persistence fails", async () => {
    await runTest(
      Effect.gen(function* () {
        const episode = {
          episodeId: `persistence-failure-${Date.now()}`,
          title: "Persistence failure",
          articleUrl: "https://example.com/article",
          content: "content",
          audioFile: `persistence-failure-${Date.now()}.mp3`,
          fileBytes: 5,
          createdAt: Date.now(),
        } satisfies PressPodsEpisodeData;

        const exit = yield* Effect.exit(
          persistEpisodeWithAudio(episode, Buffer.from("audio"), () =>
            Effect.fail(
              new PressPodsError({
                operation: "persist test episode",
                cause: new Error("database unavailable"),
              }),
            ),
          ),
        );

        expect(Exit.isFailure(exit)).toBe(true);
        const stat = yield* Effect.exit(
          Effect.tryPromise(() => fs.stat(episodeAudioPath(episode.audioFile))),
        );
        expect(Exit.isFailure(stat)).toBe(true);
        // Guard against a future path change making the assertion accidentally
        // inspect a different file name.
        expect(path.basename(episodeAudioPath(episode.audioFile))).toBe(
          episode.audioFile,
        );
      }),
    );
  });
});
