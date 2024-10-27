import { Entity } from "@micthiesen/mitools/entities";
import { Logger } from "@micthiesen/mitools/logging";

export type ChannelMetrics = {
	username: string;
	maxViewerCount: number;
};
export const ChannelMetricsEntity = new Entity<ChannelMetrics, ["username"]>(
	"channel-metrics",
	["username"],
);

const logger = new Logger("Persistence.Metrics");

export function getChannelMetrics(username: string): ChannelMetrics {
	const metrics = ChannelMetricsEntity.get({ username });

	if (metrics) {
		logger.debug(`Found metrics for ${username} in DB`, metrics);
		return metrics;
	}

	logger.debug(`No metrics found in DB for ${username}; returning default`);
	return { username, maxViewerCount: 0 };
}

export function upsertChannelMetrics(metrics: ChannelMetrics): void {
	ChannelMetricsEntity.upsert(metrics);
	logger.debug(`Upserted metrics for ${metrics.username} in DB`, metrics);
}
