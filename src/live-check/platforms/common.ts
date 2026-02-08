import got from "got";

const TIMEOUT_MS = 10_000;

export interface GQLRequestOptions {
  url: string;
  clientId: string;
  query: string;
}

export async function fetchGQL<T>(options: GQLRequestOptions): Promise<T> {
  const { url, clientId, query } = options;
  try {
    return await got
      .post(url, {
        json: { query },
        headers: { "Client-Id": clientId },
        timeout: { request: TIMEOUT_MS },
      })
      .json<T>();
  } catch (error) {
    throw new Error(`Failed to fetch GQL from ${url}: ${(error as Error)?.message}`);
  }
}

export async function fetchPageHtml(url: string): Promise<string> {
  try {
    return await got(url, { timeout: { request: TIMEOUT_MS } }).text();
  } catch (error) {
    throw new Error(
      `Failed to check live status for ${url}: ${(error as Error)?.message}`,
    );
  }
}
