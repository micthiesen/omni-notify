import { Entity } from "@micthiesen/mitools/entities";
import type { Platform } from "../../platforms/index.js";

export type ChannelStatusLive = {
  username: string;
  platform: Platform;
  isLive: true;
  title: string;
  startedAt: Date;
  maxViewerCount?: number;
};
export type ChannelStatusOffline =
  | {
      username: string;
      platform: Platform;
      isLive: false;
      lastEndedAt?: undefined;
      lastStartedAt?: undefined;
      lastViewerCount?: undefined;
    }
  | {
      username: string;
      platform: Platform;
      isLive: false;
      lastEndedAt: Date;
      lastStartedAt: Date;
      lastViewerCount?: number;
    };
export type ChannelStatus = ChannelStatusLive | ChannelStatusOffline;

export const ChannelStatusEntity = new Entity<ChannelStatus, ["username"]>(
  "channel-status",
  ["username"],
);

export function getChannelStatus(username: string, platform: Platform): ChannelStatus {
  const status = ChannelStatusEntity.get({ username });
  return status ?? { username, platform, isLive: false };
}

export function upsertChannelStatus(status: ChannelStatus): void {
  ChannelStatusEntity.upsert(status);
}
