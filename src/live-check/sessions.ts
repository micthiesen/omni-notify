import { Entity } from "@micthiesen/mitools/entities";
import type { StreamerStatusLive } from "./persistence.js";
import type { Platform } from "./platforms/index.js";

/** One completed live session (recorded when the streamer goes offline). */
export type StreamSession = {
  startedAt: number;
  endedAt: number;
  durationMs: number;
  /** Peak of the summed viewer count across live bindings. */
  peakViewers: number;
  /** Primary binding's title/platform at the time the session closed. */
  title: string;
  platform: Platform;
  username: string;
};

export type StreamSessionsData = {
  streamerId: string;
  /** Oldest → newest. */
  sessions: StreamSession[];
};

export const MAX_SESSIONS = 300;
export const MAX_SESSION_AGE_MS = 180 * 24 * 60 * 60 * 1000;

export const StreamSessionsEntity = new Entity<StreamSessionsData, ["streamerId"]>(
  "streamer-sessions",
  ["streamerId"],
);

export function getStreamSessions(streamerId: string): StreamSessionsData {
  return StreamSessionsEntity.get({ streamerId }) ?? { streamerId, sessions: [] };
}

/** Pure: append a completed session, pruning entries beyond age/count caps. */
export function appendSession(
  data: StreamSessionsData,
  session: StreamSession,
): StreamSessionsData {
  const cutoff = session.endedAt - MAX_SESSION_AGE_MS;
  const sessions = [...data.sessions, session]
    .filter((s) => s.endedAt >= cutoff)
    .slice(-MAX_SESSIONS);
  return { ...data, sessions };
}

/**
 * Pure: build the session record from the live status being closed out.
 * Entity data round-trips through JSON, so `startedAt` may be an ISO string
 * at runtime regardless of its declared Date type.
 */
export function sessionFromLiveStatus(
  live: StreamerStatusLive,
  endedAt: Date,
): StreamSession {
  const startedAt = new Date(live.startedAt).getTime();
  const endedMs = endedAt.getTime();
  return {
    startedAt,
    endedAt: endedMs,
    durationMs: Math.max(0, endedMs - startedAt),
    peakViewers: live.maxViewerCount,
    title: live.primaryTitle,
    platform: live.primary.platform,
    username: live.primary.username,
  };
}

export function recordCompletedSession(live: StreamerStatusLive, endedAt: Date): void {
  const data = getStreamSessions(live.streamerId);
  StreamSessionsEntity.upsert(
    appendSession(data, sessionFromLiveStatus(live, endedAt)),
  );
}
