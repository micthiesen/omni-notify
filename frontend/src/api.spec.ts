import { describe, expect, it } from "vitest";
import { Schema } from "effect";
import {
  LivestreamIntelligenceDetailsSchema,
  PodcastRecommendationSchema,
  PressPodsEpisodeDetailSchema,
  SnapshotSchema,
  WorkspaceDetailResponseSchema,
} from "./api";

const expectDecodeFailure = <A, I>(
  schema: Schema.Schema<A, I, never>,
  value: unknown,
): void => {
  expect(() => Schema.decodeUnknownSync(schema)(value)).toThrow();
};

describe("frontend API schemas", () => {
  it("rejects malformed nested task data in a snapshot", () => {
    expectDecodeFailure(SnapshotSchema, {
      tasks: [
        {
          name: "LiveCheck",
          schedule: "* * * * *",
          running: false,
          nextRuns: [123],
          lastRun: null,
        },
      ],
      streamers: [],
      runs: [],
      onDeck: [],
    });
  });

  it("rejects malformed nested podcast shortlist scores", () => {
    expectDecodeFailure(PodcastRecommendationSchema, {
      recommendationId: "rec_1",
      showTitle: "A Show",
      episodeTitle: "An Episode",
      feedUrl: "https://example.com/feed.xml",
      publishedAt: 1,
      status: "notified",
      shortlistScores: {
        tasteMatch: 8,
        novelty: 7,
        composite: 7.5,
        risks: [false],
      },
      recommendedAt: 2,
    });
  });

  it("rejects malformed nested PressPods cost accounting", () => {
    expectDecodeFailure(PressPodsEpisodeDetailSchema, {
      episodeId: "episode_1",
      title: "Article",
      author: null,
      publication: null,
      domain: null,
      articleUrl: "https://example.com/article",
      leadImageUrl: null,
      excerpt: null,
      voiceName: null,
      synthesizedSeconds: null,
      audioUrl: "/pods/episode_1.mp3",
      durationSeconds: null,
      fileBytes: 100,
      retrieverName: null,
      retrieverSeconds: null,
      retrieverAttempts: null,
      chapters: null,
      costCents: null,
      createdAt: 1,
      publishedAt: null,
      runId: null,
      content: "Article body",
      authorGender: null,
      voiceProvider: null,
      chunks: null,
      costs: {
        llmCents: 1,
        ttsCents: 2,
        detailCents: { metadata: 1 },
        detailTokens: { metadata: { input: 10, output: "invalid" } },
        detailChars: { speech: 100 },
      },
    });
  });

  it("rejects malformed nested workspace subject state", () => {
    expectDecodeFailure(WorkspaceDetailResponseSchema, {
      workspace: {
        id: "research",
        title: "Research",
        description: "Research workspace",
        subjectLabel: "Subject",
        subjectLabelPlural: "Subjects",
        taskName: "WorkspaceResearch",
        schedule: "0 12 * * *",
        instructions: "Research it",
        artifacts: [],
      },
      subject: {
        workspaceId: "research",
        subjectId: "subject_1",
        title: "A subject",
        status: "deleted",
        summary: "Summary",
        createdAt: 1,
        updatedAt: 2,
      },
      artifacts: [],
      artifactRevisions: [],
      messages: [],
      sources: [],
      actions: [],
      emailScope: null,
      papercuts: [],
    });
  });

  it("rejects malformed nested livestream runtime queue data", () => {
    expectDecodeFailure(LivestreamIntelligenceDetailsSchema, {
      intelligence: null,
      diagnostics: null,
      events: [],
      runtime: {
        enabled: true,
        voiceprintLoaded: true,
        model: "model",
        queues: {
          capture: { running: "one", queued: 0 },
          speech: { running: 0, queued: 0 },
          llm: { running: 0, queued: 0 },
        },
        activeStreamCount: 1,
        activeVoiceTargetCount: 1,
        budget: { spentCents: 1, limitCents: 10, remainingCents: 9 },
        intervals: { voiceSeconds: 60, summarySeconds: 300 },
      },
      generatedAt: 1,
    });
  });
});
