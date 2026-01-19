import { describe, expect, it } from "vitest";
import { extractLiveStatus } from "./twitch.js";

describe("extractLiveStatus", () => {
	it("should return live status with title and viewer count", () => {
		const data = {
			data: {
				user: {
					stream: {
						title: "Playing games",
						viewersCount: 15000,
					},
				},
			},
		};
		expect(extractLiveStatus(data)).toEqual({
			isLive: true,
			title: "Playing games",
			viewerCount: 15000,
		});
	});

	it("should return offline when stream is null", () => {
		const data = { data: { user: { stream: null } } };
		expect(extractLiveStatus(data)).toEqual({ isLive: false });
	});

	it("should return offline when user is null", () => {
		const data = { data: { user: null } };
		expect(extractLiveStatus(data)).toEqual({ isLive: false });
	});
});
