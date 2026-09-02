import type { Effect as EffectType } from "effect/Effect";
import type { Docstore } from "@micthiesen/mitools/docstore";
import { Entity } from "@micthiesen/mitools/entities";
import { Effect, Option } from "effect";
import type { PersistenceError } from "../effect/errors.js";
import { PersistenceError as PersistenceFailure } from "../effect/errors.js";
import type { PlatformBinding } from "./streamers.js";

export type StreamerStatusLive = {
  streamerId: string;
  isLive: true;
  primary: PlatformBinding;
  primaryTitle: string;
  startedAt: Date;
  maxViewerCount: number;
  /**
   * Current (not max) summed viewer count as of the most recent tick.
   * Optional: rows persisted before this field existed won't have it.
   */
  viewerCount?: number;
  /**
   * Current per-platform observations that make up viewerCount. Older rows do
   * not have this field; platform history intentionally starts when the
   * account is first observed by the new collector.
   */
  sources?: Array<{
    platform: PlatformBinding["platform"];
    username: string;
    title: string;
    viewerCount?: number;
    category?: string;
  }>;
  /**
   * The primary binding's category/game, when the platform reports one (e.g.
   * Twitch/Kick; YouTube never sets this). Optional for the same reason as
   * viewerCount.
   */
  category?: string;
};

export type StreamerStatusOffline = {
  streamerId: string;
  isLive: false;
  lastEndedAt?: Date;
  lastStartedAt?: Date;
  lastMaxViewerCount?: number;
};

export type StreamerStatus = StreamerStatusLive | StreamerStatusOffline;

export const StreamerStatusEntity = new Entity<StreamerStatus, ["streamerId"]>(
  "streamer-status",
  ["streamerId"],
);

export function getStreamerStatusEffect(
  streamerId: string,
): EffectType<StreamerStatus, PersistenceError, Docstore> {
  return StreamerStatusEntity.get({ streamerId }).pipe(
    Effect.map(Option.getOrElse((): StreamerStatus => ({ streamerId, isLive: false }))),
    Effect.mapError(
      (cause) => new PersistenceFailure({ operation: "read streamer status", cause }),
    ),
  );
}

export function upsertStreamerStatusEffect(
  status: StreamerStatus,
): EffectType<void, PersistenceError, Docstore> {
  return StreamerStatusEntity.upsert(status).pipe(
    Effect.mapError(
      (cause) => new PersistenceFailure({ operation: "upsert streamer status", cause }),
    ),
  );
}
