import { describe, expect, it } from "vitest";
import type { StreamerStatusLive } from "./persistence.js";
import { Platform } from "./platforms/index.js";
import {
  appendSession,
  MAX_SESSION_AGE_MS,
  MAX_SESSIONS,
  type StreamSession,
  type StreamSessionsData,
  sessionFromLiveStatus,
} from "./sessions.js";

const HOUR = 60 * 60 * 1000;

function makeSession(overrides: Partial<StreamSession> = {}): StreamSession {
  return {
    startedAt: 1_000_000,
    endedAt: 1_000_000 + HOUR,
    durationMs: HOUR,
    peakViewers: 123,
    title: "Test stream",
    platform: Platform.Twitch,
    username: "tester",
    ...overrides,
  };
}

describe("sessionFromLiveStatus", () => {
  const base: StreamerStatusLive = {
    streamerId: "tester",
    isLive: true,
    primary: { platform: Platform.YouTube, username: "@tester" },
    primaryTitle: "Big stream",
    startedAt: new Date(1_000_000),
    maxViewerCount: 456,
  };

  it("builds a completed session from live status", () => {
    const session = sessionFromLiveStatus(base, new Date(1_000_000 + HOUR));
    expect(session).toEqual({
      startedAt: 1_000_000,
      endedAt: 1_000_000 + HOUR,
      durationMs: HOUR,
      peakViewers: 456,
      title: "Big stream",
      platform: Platform.YouTube,
      username: "@tester",
    });
  });

  it("handles startedAt as an ISO string (JSON round-trip)", () => {
    const roundTripped = {
      ...base,
      startedAt: new Date(1_000_000).toISOString() as unknown as Date,
    };
    const session = sessionFromLiveStatus(roundTripped, new Date(1_000_000 + HOUR));
    expect(session.startedAt).toBe(1_000_000);
    expect(session.durationMs).toBe(HOUR);
  });

  it("clamps negative durations to zero", () => {
    const session = sessionFromLiveStatus(base, new Date(500_000));
    expect(session.durationMs).toBe(0);
  });
});

describe("appendSession", () => {
  const empty: StreamSessionsData = { streamerId: "tester", sessions: [] };

  it("appends to an empty list", () => {
    const result = appendSession(empty, makeSession());
    expect(result.sessions).toHaveLength(1);
  });

  it("keeps sessions ordered oldest to newest", () => {
    const first = makeSession({ endedAt: 1_000_000 + HOUR });
    const second = makeSession({
      startedAt: 2_000_000,
      endedAt: 2_000_000 + HOUR,
    });
    const result = appendSession(appendSession(empty, first), second);
    expect(result.sessions.map((s) => s.endedAt)).toEqual([
      1_000_000 + HOUR,
      2_000_000 + HOUR,
    ]);
  });

  it("prunes sessions older than the age cap", () => {
    const now = MAX_SESSION_AGE_MS + 10 * HOUR;
    const stale = makeSession({ startedAt: 0, endedAt: HOUR });
    const fresh = makeSession({ startedAt: now - HOUR, endedAt: now });
    const result = appendSession({ ...empty, sessions: [stale] }, fresh);
    expect(result.sessions).toEqual([fresh]);
  });

  it("caps the list at MAX_SESSIONS", () => {
    const sessions = Array.from({ length: MAX_SESSIONS }, (_, i) =>
      makeSession({ startedAt: i * HOUR, endedAt: i * HOUR + HOUR }),
    );
    const newest = makeSession({
      startedAt: MAX_SESSIONS * HOUR,
      endedAt: (MAX_SESSIONS + 1) * HOUR,
    });
    const result = appendSession({ ...empty, sessions }, newest);
    expect(result.sessions).toHaveLength(MAX_SESSIONS);
    expect(result.sessions.at(-1)).toEqual(newest);
    expect(result.sessions[0]?.startedAt).toBe(HOUR);
  });
});
