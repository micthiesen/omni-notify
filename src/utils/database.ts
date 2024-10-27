import { Logger } from "@micthiesen/mitools";
import Database from "better-sqlite3";

export type ChannelStatusLive = {
	username: string;
	isLive: true;
	title: string;
	startedAt: Date;
	maxViewerCount?: number;
};

export type ChannelStatusOffline =
	| {
			username: string;
			isLive: false;
			lastEndedAt?: undefined;
			lastStartedAt?: undefined;
			lastViewerCount?: undefined;
	  }
	| {
			username: string;
			isLive: false;
			lastEndedAt: Date;
			lastStartedAt: Date;
			lastViewerCount?: number;
	  };

export type ChannelStatus = ChannelStatusLive | ChannelStatusOffline;

const dateKeys = new Set(["startedAt", "lastEndedAt", "lastStartedAt"]);

const db = new Database("statuses.db");

db.pragma("journal_mode = WAL");
db.pragma("synchronous = NORMAL");

db.prepare(`
  CREATE TABLE IF NOT EXISTS statuses (
    username TEXT PRIMARY KEY,
    data     JSON
  )
`).run();

const logger = new Logger("database");

/**
 * Retrieves the channel status for a given username
 *
 * @param username - The username of the channel
 * @returns The ChannelStatus object or a default offline status if not found
 */
export function getChannelStatus(username: string): ChannelStatus {
	const stmt = db.prepare("SELECT data FROM statuses WHERE username = ?");
	const row = stmt.get(username) as { data: string } | undefined;
	if (row) {
		return JSON.parse(row.data, (key, value) => {
			return dateKeys.has(key) && typeof value === "string" ? new Date(value) : value;
		});
	}

	logger.debug(`No status found in DB for ${username}; returning default`);
	return { username, isLive: false };
}

/**
 * Upserts the channel status for a given username
 *
 * @param status - The ChannelStatus object to upsert
 */
export function upsertChannelStatus(status: ChannelStatus): void {
	const stmt = db.prepare(`
    INSERT INTO statuses (username, data)
    VALUES (?, json(?))
    ON CONFLICT(username) DO UPDATE SET data=excluded.data
  `);

	const data = JSON.stringify(status, (_, value) =>
		value instanceof Date ? value.toISOString() : value,
	);
	stmt.run(status.username, data);
	logger.debug(`Upserted status for ${status.username} in DB`);
}
