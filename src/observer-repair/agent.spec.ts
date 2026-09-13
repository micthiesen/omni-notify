import { describe, expect, it } from "vitest";
import { validateDecision } from "./agent.js";
import type { ObserverIssue } from "../observer/client.js";

function issue(overrides: Partial<ObserverIssue> = {}): ObserverIssue {
  return {
    id: 10,
    issueType: 1,
    status: 1,
    problemSeason: 3,
    problemEpisode: 1,
    createdAt: "2026-01-01T00:00:00.000Z",
    updatedAt: "2026-01-01T00:00:00.000Z",
    media: { id: 44, mediaType: "tv", tmdbId: 123, tvdbId: 456 },
    comments: [{ id: 1, message: "The episode freezes", user: null }],
    ...overrides,
  };
}

function decision(overrides: Record<string, unknown> = {}) {
  return {
    action: "replace",
    season: 3,
    episodes: [1],
    scopeComment: null,
    reason: "Replace the faulty file",
    ...overrides,
  };
}

describe("validateDecision", () => {
  it("accepts a replacement at the reported episode scope", () => {
    expect(validateDecision(issue(), decision())).toMatchObject({
      action: "replace",
      season: 3,
      episodes: [1],
    });
  });

  it("rejects a decision that broadens a season or episode", () => {
    expect(() => validateDecision(issue(), decision({ season: 4 }))).toThrow(
      "Scope exceeds the reported season",
    );
    expect(() => validateDecision(issue(), decision({ episodes: [2] }))).toThrow(
      "Scope exceeds the reported episode",
    );
    expect(() => validateDecision(issue(), decision({ episodes: [1, 2] }))).toThrow(
      "Scope exceeds the reported episode",
    );
  });

  it("allows a narrower episode list only with an exact supporting comment", () => {
    const allSeason = issue({ problemSeason: 3, problemEpisode: 0 });
    expect(() => validateDecision(allSeason, decision({ episodes: [1] }))).toThrow(
      "Narrowed scope requires an exact supporting issue comment",
    );
    expect(
      validateDecision(
        allSeason,
        decision({ episodes: [1], scopeComment: "The episode freezes" }),
      ),
    ).toMatchObject({ episodes: [1], scopeComment: "The episode freezes" });
    expect(() =>
      validateDecision(
        allSeason,
        decision({ episodes: [1], scopeComment: "The episode freezes on startup" }),
      ),
    ).toThrow("Narrowed scope requires an exact supporting issue comment");
  });

  it("accepts full series scope and rejects unknown media", () => {
    expect(
      validateDecision(
        issue({ problemSeason: 0, problemEpisode: 0 }),
        decision({ season: null, episodes: [] }),
      ),
    ).toMatchObject({ season: null, episodes: [] });
    expect(() =>
      validateDecision(issue({ media: { id: 44, mediaType: "music" } }), decision()),
    ).toThrow("Unknown media type");
  });

  it("rejects TV scope for movies", () => {
    const movie = issue({ media: { id: 44, mediaType: "movie", tmdbId: 123 } });
    expect(
      validateDecision(movie, decision({ season: null, episodes: [] })),
    ).toMatchObject({
      action: "replace",
      season: null,
      episodes: [],
    });
    expect(() => validateDecision(movie, decision())).toThrow(
      "Movie scope cannot contain episodes",
    );
  });

  it("rejects duplicate episodes and permits cannot_handle", () => {
    expect(() =>
      validateDecision(issue({ problemEpisode: 0 }), decision({ episodes: [1, 1] })),
    ).toThrow("Duplicate episodes in scope");
    expect(
      validateDecision(
        issue(),
        decision({ action: "cannot_handle", season: null, episodes: [] }),
      ),
    ).toMatchObject({ action: "cannot_handle" });
  });
});
