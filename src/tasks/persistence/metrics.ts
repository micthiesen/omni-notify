import { Entity } from "@micthiesen/mitools/entities";

export type ChannelMetrics = {
  username: string;
  maxViewerCount: number;
};
export const ChannelMetricsEntity = new Entity<ChannelMetrics, ["username"]>(
  "channel-metrics",
  ["username"],
);

export function getChannelMetrics(username: string): ChannelMetrics {
  const metrics = ChannelMetricsEntity.get({ username });
  return metrics ?? { username, maxViewerCount: 0 };
}

export function upsertChannelMetrics(metrics: ChannelMetrics): void {
  ChannelMetricsEntity.upsert(metrics);
}
