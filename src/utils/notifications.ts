import https from "node:https";
import { URLSearchParams } from "node:url";
import config from "./config.js";

interface PushoverMessage {
	message: string;
	title: string;
	url?: string;
	url_title?: string;
	priority?: number;
	sound?: string;
	timestamp?: number;
}

export async function sendNotification(
	message: PushoverMessage,
): Promise<void> {
	return new Promise((resolve, reject) => {
		const params = new URLSearchParams({
			token: config.PUSHOVER_APP_TOKEN,
			user: config.PUSHOVER_USER_KEY,
			message: message.message,
			...(message.title && { title: message.title }),
			...(message.url && { url: message.url }),
			...(message.url_title && { url_title: message.url_title }),
			...(message.priority !== undefined && {
				priority: message.priority.toString(),
			}),
			...(message.sound && { sound: message.sound }),
			...(message.timestamp && { timestamp: message.timestamp.toString() }),
		});

		const options = {
			hostname: "api.pushover.net",
			port: 443,
			path: "/1/messages.json",
			method: "POST",
			headers: {
				"Content-Type": "application/x-www-form-urlencoded",
				"Content-Length": params.toString().length,
			},
		};

		const req = https.request(options, (res) => {
			let data = "";

			res.on("data", (chunk) => {
				data += chunk;
			});

			res.on("end", () => {
				if (res.statusCode === 200) {
					resolve();
				} else {
					reject(
						new Error(
							`Pushover API returned status code ${res.statusCode}: ${data}`,
						),
					);
				}
			});
		});

		req.on("error", (error) => {
			reject(error);
		});

		req.write(params.toString());
		req.end();
	});
}
