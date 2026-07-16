import { describe, expect, it } from "vitest";
import { applyGuestDecisions, type GuestDecision } from "./guestSelection.js";
import type { EpisodeCandidate } from "./types.js";

function candidate(id: string): EpisodeCandidate {
  return {
    episodeId: id,
    showId: "itunes:1",
    showTitle: `Show ${id}`,
    episodeTitle: `Ep ${id}`,
    feedUrl: "https://f",
    episodeGuid: "g",
    publishedAt: 0,
    description: "",
    showGenres: [],
    discoveredVia: "guest",
  };
}

// Minimal decision matching the guest-gate schema; overridable per test.
function decision(over: Partial<GuestDecision>): GuestDecision {
  return {
    candidate_id: "e1",
    include: true,
    reason: "",
    why_for_user: "why",
    caveats: [],
    confidence: 0.5,
    notification: { title: "t", message: "m" },
    ...over,
  };
}

const byId = (...ids: string[]) =>
  new Map(ids.map((id) => [id, candidate(id)] as const));

describe("applyGuestDecisions", () => {
  it("keeps includes and drops excludes", () => {
    const picks = applyGuestDecisions(
      [
        decision({ candidate_id: "e1" }),
        decision({ candidate_id: "e2", include: false }),
      ],
      byId("e1", "e2"),
      5,
    );
    expect(picks.map((p) => p.candidate.episodeId)).toEqual(["e1"]);
  });

  it("dedups a repeated candidate_id (no double-commit)", () => {
    const picks = applyGuestDecisions(
      [decision({ candidate_id: "e1" }), decision({ candidate_id: "e1" })],
      byId("e1"),
      5,
    );
    expect(picks).toHaveLength(1);
  });

  it("keeps the strongest when over max, regardless of array order", () => {
    const picks = applyGuestDecisions(
      [
        decision({ candidate_id: "e1", confidence: 0.2 }),
        decision({ candidate_id: "e2", confidence: 0.9 }),
        decision({ candidate_id: "e3", confidence: 0.5 }),
      ],
      byId("e1", "e2", "e3"),
      2,
    );
    expect(picks.map((p) => p.candidate.episodeId)).toEqual(["e2", "e3"]);
  });

  it("skips unknown candidate ids", () => {
    const picks = applyGuestDecisions(
      [decision({ candidate_id: "nope" })],
      byId("e1"),
      5,
    );
    expect(picks).toHaveLength(0);
  });

  it("skips includes missing why_for_user or notification", () => {
    const picks = applyGuestDecisions(
      [
        decision({ candidate_id: "e1", why_for_user: null }),
        decision({ candidate_id: "e2", notification: null }),
      ],
      byId("e1", "e2"),
      5,
    );
    expect(picks).toHaveLength(0);
  });
});
