import { Entity } from "@micthiesen/mitools/entities";
import type { Platform } from "./platforms/index.js";

export type StreamerLiveBinding = {
  platform: Platform;
  username: string;
  title: string;
  viewerCount?: number;
};

export type StreamerStatusLive = {
  streamerId: string;
  isLive: true;
  primary: { platform: Platform; username: string };
  primaryTitle: string;
  startedAt: Date;
  maxViewerCount: number;
  bindings: StreamerLiveBinding[];
};

export type StreamerStatusOffline =
  | { streamerId: string; isLive: false; lastEndedAt?: undefined }
  | {
      streamerId: string;
      isLive: false;
      lastEndedAt: Date;
      lastStartedAt: Date;
      lastMaxViewerCount?: number;
    };

export type StreamerStatus = StreamerStatusLive | StreamerStatusOffline;

export const StreamerStatusEntity = new Entity<StreamerStatus, ["streamerId"]>(
  "streamer-status",
  ["streamerId"],
);

export function getStreamerStatus(streamerId: string): StreamerStatus {
  return StreamerStatusEntity.get({ streamerId }) ?? { streamerId, isLive: false };
}

export function upsertStreamerStatus(status: StreamerStatus): void {
  StreamerStatusEntity.upsert(status);
}
