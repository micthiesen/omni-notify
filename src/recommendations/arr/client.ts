export interface ArrConfig {
  url?: string;
  apiKey?: string;
  rootFolderPath?: string;
  qualityProfileId?: number;
}

export type HttpResult<T> =
  | { status: "ok"; value: T }
  | { status: "http_error"; statusCode: number }
  | { status: "unavailable" };

export type FetchImplementation = typeof fetch;

export type ArrConnectionConfig = Required<Pick<ArrConfig, "url" | "apiKey">>;

export function hasArrConnection(
  config: ArrConfig,
): config is ArrConfig & ArrConnectionConfig {
  return Boolean(config.url && config.apiKey);
}

export function isConfigured(config: ArrConfig): config is Required<ArrConfig> {
  return Boolean(
    config.url &&
      config.apiKey &&
      config.rootFolderPath &&
      Number.isInteger(config.qualityProfileId),
  );
}

export async function requestJson<T>(
  config: ArrConnectionConfig,
  path: string,
  init: RequestInit = {},
  fetchImpl: FetchImplementation = fetch,
): Promise<HttpResult<T>> {
  const url = new URL(
    `api/v3/${path.replace(/^\//, "")}`,
    `${config.url.replace(/\/+$/, "")}/`,
  );

  try {
    const response = await fetchImpl(url, {
      ...init,
      headers: {
        Accept: "application/json",
        "X-Api-Key": config.apiKey,
        ...init.headers,
      },
      signal: init.signal ?? AbortSignal.timeout(15_000),
    });
    if (!response.ok) {
      return { status: "http_error", statusCode: response.status };
    }
    return { status: "ok", value: (await response.json()) as T };
  } catch {
    return { status: "unavailable" };
  }
}

export function postJson(value: unknown): RequestInit {
  return {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(value),
  };
}

export function optionalNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

export function optionalString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}
