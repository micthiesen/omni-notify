import { Data, Effect, Schema } from "effect";

const USER_AGENT = "OpenAI File Downloader, XaiImageApiFetch/1.0";
const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;
const REQUEST_TIMEOUT_MS = 20_000;

export class HisterError extends Data.TaggedError("HisterError")<{
  readonly operation: string;
  readonly reason: string;
}> {
  public override get message(): string {
    return `${this.operation}: ${this.reason}`;
  }
}

export interface HisterSearchInput {
  readonly query: string;
  readonly limit?: number;
  readonly cursor?: string;
  readonly dateFrom?: string;
  readonly dateTo?: string;
  readonly semantic?: boolean;
}

export interface HisterSearchResult {
  readonly documentId?: string;
  readonly title: string;
  readonly titleTruncated: boolean;
  readonly url: string;
  readonly snippet: string;
  readonly snippetTruncated: boolean;
  readonly label?: string;
  readonly domain?: string;
  readonly updatedAt?: number;
  readonly score?: number;
}

export interface HisterSearchResponse {
  readonly results: readonly HisterSearchResult[];
  readonly priorSelections: readonly HisterSearchResult[];
  readonly priorSelectionsNote: string;
  readonly total: number;
  readonly nextCursor?: string;
}

export interface HisterPageResponse {
  readonly documentId?: string;
  readonly title: string;
  readonly titleTruncated: boolean;
  readonly url: string;
  readonly text: string;
  readonly totalChars: number;
  readonly offset: number;
  readonly nextOffset?: number;
}

export interface HisterBrowseInput {
  readonly filter?: string;
  readonly dateFrom?: number;
  readonly dateTo?: number;
  readonly cursor?: string;
}

export interface HisterBrowseItem {
  readonly documentId?: string;
  readonly title: string;
  readonly titleTruncated: boolean;
  readonly url: string;
  readonly updatedAt: number;
  readonly indexedVersions?: number;
}

export interface HisterBrowseResponse {
  readonly items: readonly HisterBrowseItem[];
  readonly nextCursor?: string;
}

export interface HisterService {
  readonly searchEffect: (
    input: HisterSearchInput,
  ) => Effect.Effect<HisterSearchResponse, HisterError>;
  readonly getPageEffect: (input: {
    readonly url: string;
    readonly documentId?: string;
    readonly offset?: number;
    readonly maxChars?: number;
  }) => Effect.Effect<HisterPageResponse, HisterError>;
  readonly browseEffect: (
    input: HisterBrowseInput,
  ) => Effect.Effect<HisterBrowseResponse, HisterError>;
  readonly setLabelEffect: (input: {
    readonly url: string;
    readonly label: string;
  }) => Effect.Effect<
    { readonly url: string; readonly label: string; readonly verified: true },
    HisterError
  >;
}

export type HisterFetch = typeof fetch;

const storedUrl = Schema.String.pipe(
  Schema.check(Schema.isMinLength(1), Schema.isMaxLength(8_192)),
);
const documentId = Schema.String.pipe(Schema.check(Schema.isMaxLength(8_256)));
const pageKey = Schema.String.pipe(Schema.check(Schema.isMaxLength(16_384)));
const nonNegative = Schema.Finite.pipe(Schema.check(Schema.isGreaterThanOrEqualTo(0)));
const SearchDocument = Schema.Struct({
  id: Schema.optional(documentId),
  url: storedUrl,
  title: Schema.String,
  text: Schema.optional(Schema.String),
  snippet: Schema.optional(Schema.String),
  label: Schema.optional(Schema.String.pipe(Schema.check(Schema.isMaxLength(4_096)))),
  domain: Schema.optional(Schema.String.pipe(Schema.check(Schema.isMaxLength(1_024)))),
  updated: Schema.optional(nonNegative),
  score: Schema.optional(Schema.Finite),
});
const SearchResponse = Schema.Struct({
  total: nonNegative,
  documents: Schema.Array(SearchDocument).pipe(Schema.check(Schema.isMaxLength(50))),
  history: Schema.optional(
    Schema.NullOr(
      Schema.Array(SearchDocument).pipe(Schema.check(Schema.isMaxLength(20))),
    ),
  ),
  page_key: Schema.optional(pageKey),
});
const DocumentResponse = Schema.Struct({
  id: Schema.optional(documentId),
  url: storedUrl,
  title: Schema.optional(Schema.String),
  text: Schema.optional(Schema.String),
  label: Schema.optional(Schema.String),
});
const IndexedHistoryResponse = Schema.Struct({
  documents: Schema.Array(
    Schema.Struct({
      id: Schema.optional(documentId),
      url: storedUrl,
      title: Schema.String,
      updated: nonNegative,
      add_count: Schema.optional(nonNegative),
    }),
  ).pipe(Schema.check(Schema.isMaxLength(100))),
  page_key: Schema.optional(pageKey),
});
const IndexedHistoryPayload = Schema.Union([IndexedHistoryResponse, Schema.Null]);

const decode = <A>(
  schema: Schema.ConstraintDecoder<A>,
  value: unknown,
  operation: string,
): Effect.Effect<A, HisterError> =>
  Effect.try({
    try: () => Schema.decodeUnknownSync(schema)(value),
    catch: () => new HisterError({ operation, reason: "invalid response" }),
  });

function dateToUnix(value: string, end: boolean): number {
  const date = new Date(`${value}T00:00:00.000Z`);
  if (Number.isNaN(date.getTime()) || date.toISOString().slice(0, 10) !== value)
    throw new Error("invalid date");
  return Math.floor(date.getTime() / 1000) + (end ? 86_399 : 0);
}

function boundedLimit(limit: number | undefined, fallback: number): number {
  if (limit === undefined) return fallback;
  if (!Number.isInteger(limit) || limit < 1 || limit > 50) {
    throw new Error("limit must be an integer between 1 and 50");
  }
  return limit;
}

function boundedChars(value: number | undefined): number {
  if (value === undefined) return 20_000;
  if (!Number.isInteger(value) || value < 1 || value > 50_000) {
    throw new Error("maxChars must be an integer between 1 and 50000");
  }
  return value;
}

function boundedText(
  value: string,
  max: number,
): { value: string; truncated: boolean } {
  return value.length > max
    ? { value: value.slice(0, max), truncated: true }
    : { value, truncated: false };
}

async function readBody(response: Response): Promise<unknown> {
  if (!response.body) return JSON.parse(await response.text());
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const next = await reader.read();
      if (next.done) break;
      size += next.value.byteLength;
      if (size > MAX_RESPONSE_BYTES) throw new Error("response too large");
      chunks.push(next.value);
    }
  } finally {
    await reader.cancel();
    reader.releaseLock();
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return JSON.parse(new TextDecoder().decode(bytes));
}

function searchResult(
  document: Schema.Schema.Type<typeof SearchDocument>,
): HisterSearchResult {
  const snippet = boundedText(document.snippet ?? document.text ?? "", 1_500);
  const title = boundedText(document.title, 1_000);
  return {
    ...(document.id ? { documentId: document.id } : {}),
    title: title.value,
    titleTruncated: title.truncated,
    url: document.url,
    snippet: snippet.value,
    snippetTruncated: snippet.truncated,
    ...(document.label ? { label: document.label } : {}),
    ...(document.domain ? { domain: document.domain } : {}),
    ...(document.updated !== undefined ? { updatedAt: document.updated } : {}),
    ...(document.score !== undefined ? { score: document.score } : {}),
  };
}

export function createHisterService(
  baseUrl: string,
  accessToken: string,
  fetchImpl: HisterFetch = fetch,
): HisterService {
  const origin = new URL(baseUrl);
  if (
    !["http:", "https:"].includes(origin.protocol) ||
    origin.username ||
    origin.password ||
    origin.search ||
    origin.hash
  ) {
    throw new Error(
      "Hister URL must use HTTP(S) without credentials, query, or fragment",
    );
  }
  const root = origin.toString().replace(/\/$/, "");

  const request = Effect.fn("Hister.request")(
    (operation: string, path: string, init?: RequestInit) =>
      Effect.tryPromise({
        try: async (signal) => {
          const response = await fetchImpl(`${root}${path}`, {
            ...init,
            redirect: "error",
            signal,
            headers: {
              Accept: "application/json",
              "User-Agent": USER_AGENT,
              "X-Access-Token": accessToken,
              ...init?.headers,
            },
          });
          if (!response.ok) {
            await response.body?.cancel();
            throw new HisterError({ operation, reason: `HTTP ${response.status}` });
          }
          return await readBody(response);
        },
        catch: (error) =>
          error instanceof HisterError
            ? error
            : new HisterError({ operation, reason: "request failed" }),
      }).pipe(
        Effect.timeout(`${REQUEST_TIMEOUT_MS} millis`),
        Effect.mapError((error) =>
          error instanceof HisterError
            ? error
            : new HisterError({ operation, reason: "request timed out" }),
        ),
      ),
  );

  const searchEffect: HisterService["searchEffect"] = (input: HisterSearchInput) =>
    Effect.try({
      try: () => {
        const limit = boundedLimit(input.limit, 10);
        const query: Record<string, unknown> = {
          text: input.query,
          limit,
          include_html: false,
          include_text: true,
          semantic_enabled: input.semantic ?? false,
        };
        if (input.cursor) query.page_key = input.cursor;
        if (input.dateFrom) query.date_from = dateToUnix(input.dateFrom, false);
        if (input.dateTo) query.date_to = dateToUnix(input.dateTo, true);
        return query;
      },
      catch: () => new HisterError({ operation: "search", reason: "invalid request" }),
    }).pipe(
      Effect.flatMap((query) =>
        request(
          "search",
          `/search?format=json&query=${encodeURIComponent(JSON.stringify(query))}`,
        ),
      ),
      Effect.flatMap((raw) => decode(SearchResponse, raw, "search")),
      Effect.map((response) => ({
        results: response.documents.map(searchResult),
        // Hister moves previously selected matching hits out of documents. Keep
        // these separately: it also returns selections outside today's filters.
        priorSelections: (response.history ?? []).map(searchResult),
        priorSelectionsNote:
          "Previously selected for this query; not constrained by current date filters. May include matching hits omitted from results. Total and cursor describe index matches, not this separate list.",
        total: response.total,
        ...(response.page_key ? { nextCursor: response.page_key } : {}),
      })),
    );

  const getPageEffect: HisterService["getPageEffect"] = (input: {
    readonly url: string;
    readonly documentId?: string;
    readonly offset?: number;
    readonly maxChars?: number;
  }) =>
    Effect.try({
      try: () => {
        const offset = input.offset ?? 0;
        const maxChars = boundedChars(input.maxChars);
        if (!Number.isInteger(offset) || offset < 0) throw new Error("invalid offset");
        return { offset, maxChars };
      },
      catch: () =>
        new HisterError({ operation: "get page", reason: "invalid request" }),
    }).pipe(
      Effect.flatMap(({ offset, maxChars }) => {
        const params = new URLSearchParams({ url: input.url });
        if (input.documentId) params.set("document_id", input.documentId);
        return request("get page", `/api/document?${params}`).pipe(
          Effect.flatMap((raw) => decode(DocumentResponse, raw, "get page")),
          Effect.map((document) => {
            const text = document.text ?? "";
            const end = Math.min(text.length, offset + maxChars);
            return {
              ...(document.id ? { documentId: document.id } : {}),
              title: boundedText(document.title ?? document.url, 1_000).value,
              titleTruncated: (document.title ?? document.url).length > 1_000,
              url: document.url,
              text: text.slice(offset, end),
              totalChars: text.length,
              offset,
              ...(end < text.length ? { nextOffset: end } : {}),
            };
          }),
        );
      }),
    );

  const browseEffect: HisterService["browseEffect"] = (input: HisterBrowseInput) =>
    request(
      "browse history",
      `/api/history?${new URLSearchParams({
        ...(input.filter ? { filter: input.filter } : {}),
        ...(input.dateFrom !== undefined ? { date_from: String(input.dateFrom) } : {}),
        ...(input.dateTo !== undefined ? { date_to: String(input.dateTo) } : {}),
        ...(input.cursor ? { last: input.cursor } : {}),
      })}`,
    ).pipe(
      Effect.flatMap((raw) => decode(IndexedHistoryPayload, raw, "browse history")),
      Effect.map((response) => ({
        items: (response?.documents ?? []).map((document) => ({
          ...(document.id ? { documentId: document.id } : {}),
          title: boundedText(document.title, 1_000).value,
          titleTruncated: document.title.length > 1_000,
          url: document.url,
          updatedAt: document.updated,
          ...(document.add_count !== undefined
            ? { indexedVersions: document.add_count }
            : {}),
        })),
        ...(response?.page_key ? { nextCursor: response.page_key } : {}),
      })),
    );

  const setLabelEffect: HisterService["setLabelEffect"] = (input: {
    readonly url: string;
    readonly label: string;
  }) =>
    request("set label", "/api/label", {
      method: "POST",
      headers: { "Content-Type": "application/json", Origin: "hister://" },
      body: JSON.stringify({ url: input.url, label: input.label }),
    }).pipe(
      Effect.flatMap((raw) =>
        decode(Schema.Struct({ ok: Schema.Literal(true) }), raw, "set label"),
      ),
      Effect.flatMap(() =>
        request(
          "verify label",
          `/api/document?url=${encodeURIComponent(input.url)}`,
        ).pipe(
          Effect.flatMap((raw) => decode(DocumentResponse, raw, "verify label")),
          Effect.flatMap((document) =>
            document.url === input.url && (document.label ?? "") === input.label
              ? Effect.succeed({
                  url: input.url,
                  label: input.label,
                  verified: true as const,
                })
              : Effect.fail(
                  new HisterError({
                    operation: "verify label",
                    reason: "label mismatch",
                  }),
                ),
          ),
        ),
      ),
    );

  return { searchEffect, getPageEffect, browseEffect, setLabelEffect };
}
