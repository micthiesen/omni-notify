const TIMEOUT_MS = 10 * 1000;

export async function fetchPageHtml(url: string): Promise<string> {
	const controller = new AbortController();
	const timeoutId = setTimeout(() => controller.abort(), TIMEOUT_MS);

	try {
		const response = await fetch(url, { signal: controller.signal });
		clearTimeout(timeoutId);

		if (!response.ok) {
			const body = await response.text();
			throw new Error(body);
		}

		return await response.text();
	} catch (error) {
		throw new Error(
			`Failed to check live status for ${url}: ${
				// biome-ignore lint/suspicious/noExplicitAny: Intentional
				(error as any)?.message
			}`,
		);
	} finally {
		clearTimeout(timeoutId);
	}
}
