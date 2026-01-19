const TIMEOUT_MS = 10 * 1000;

export interface GQLRequestOptions {
	url: string;
	clientId: string;
	query: string;
}

export async function fetchGQL<T>(options: GQLRequestOptions): Promise<T> {
	const { url, clientId, query } = options;
	const controller = new AbortController();
	const timeoutId = setTimeout(() => controller.abort(), TIMEOUT_MS);

	try {
		const response = await fetch(url, {
			method: "POST",
			headers: {
				"Client-Id": clientId,
				"Content-Type": "application/json",
			},
			body: JSON.stringify({ query }),
			signal: controller.signal,
		});
		clearTimeout(timeoutId);

		if (!response.ok) {
			const body = await response.text();
			throw new Error(body);
		}

		return await response.json();
	} catch (error) {
		throw new Error(
			`Failed to fetch GQL from ${url}: ${(error as Error)?.message}`,
		);
	} finally {
		clearTimeout(timeoutId);
	}
}

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
