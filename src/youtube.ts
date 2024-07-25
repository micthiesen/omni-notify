const TIMEOUT_MS = 10 * 1000;

export async function checkYouTubeLiveStatus({
	username,
}: { username: string }): Promise<boolean> {
	const url = `https://www.youtube.com/${username}/live`;
	const controller = new AbortController();
	const timeoutId = setTimeout(() => controller.abort(), TIMEOUT_MS);

	try {
		const response = await fetch(url, { signal: controller.signal });
		clearTimeout(timeoutId);

		if (!response.ok) {
			const body = await response.text();
			throw new Error(body);
		}

		const body = await response.text();
		return body.includes('name="title"');
	} catch (error) {
		clearTimeout(timeoutId);
		throw new Error(
			`Failed to check YouTube live status: ${
				// biome-ignore lint/suspicious/noExplicitAny: <explanation>
				(error as any)?.message
			}`,
		);
	}
}

export function getYouTubeLiveUrl(username: string) {
	return `https://www.youtube.com/${username}/live`;
}
