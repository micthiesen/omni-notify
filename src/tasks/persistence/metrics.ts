import { getDoc, upsertDoc } from "@micthiesen/mitools/docstore";
import { Logger } from "@micthiesen/mitools/logging";

export type ChannelMetrics = {
	username: string;
	maxViewerCount: number;
};

const logger = new Logger("Persistence.Metrics");

export function getChannelMetrics(username: string): ChannelMetrics {
	const pk = metricsPk(username);
	const metrics = getDoc<ChannelMetrics>(pk);

	if (metrics) {
		logger.debug(`Found metrics for ${pk} in DB`, metrics);
		return metrics;
	}

	logger.debug(`No metrics found in DB for ${pk}; returning default`);
	return { username, maxViewerCount: 0 };
}

export function upsertChannelMetrics(metrics: ChannelMetrics): void {
	const pk = metricsPk(metrics.username);
	upsertDoc<ChannelMetrics>(pk, metrics);
	logger.debug(`Upserted metrics for ${pk} in DB`, metrics);
}

function metricsPk(username: string) {
	return `$channel-metrics#${username}`;
}
