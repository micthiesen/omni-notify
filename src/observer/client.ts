import { Data, Effect, Schema } from "effect";
import { readFetchResponseTextWithLimit } from "../effect/publicHttp.js";

const MAX_RESPONSE_BYTES = 2 * 1024 * 1024;
const PAGE_SIZE = 100;
const MAX_PAGES = 20;
const MAX_RECORDS = PAGE_SIZE * MAX_PAGES;

export class ObserverClientError extends Data.TaggedError("ObserverClientError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error
        ? this.cause.message
        : typeof this.cause === "string"
          ? this.cause
          : "Observer request failed";
    return `${this.operation} failed: ${detail}`;
  }
}

const OptionalNumber = Schema.optional(Schema.NullOr(Schema.Number));
const OptionalString = Schema.optional(Schema.NullOr(Schema.String));

const UserSchema = Schema.Struct({
  id: OptionalNumber,
  username: OptionalString,
  displayName: OptionalString,
});

const MediaInfoSchema = Schema.Struct({
  id: OptionalNumber,
  tmdbId: OptionalNumber,
  tvdbId: OptionalNumber,
  status: OptionalNumber,
  mediaType: OptionalString,
  title: OptionalString,
  externalServiceId: OptionalNumber,
  externalServiceId4k: OptionalNumber,
  externalServiceSlug: OptionalString,
  externalServiceSlug4k: OptionalString,
  serviceId: OptionalNumber,
  serviceId4k: OptionalNumber,
  serviceUrl: OptionalString,
});

const IssueCommentSchema = Schema.Struct({
  id: Schema.Number,
  message: Schema.String,
  user: Schema.optional(Schema.NullOr(UserSchema)),
  createdAt: OptionalString,
  updatedAt: OptionalString,
});

const IssueSchema = Schema.Struct({
  id: Schema.Number,
  issueType: Schema.Number,
  status: Schema.Number,
  problemSeason: OptionalNumber,
  problemEpisode: OptionalNumber,
  createdAt: OptionalString,
  updatedAt: OptionalString,
  media: Schema.optional(Schema.NullOr(MediaInfoSchema)),
  createdBy: Schema.optional(Schema.NullOr(UserSchema)),
  modifiedBy: Schema.optional(Schema.NullOr(UserSchema)),
  comments: Schema.optional(Schema.NullOr(Schema.Array(IssueCommentSchema))),
});

const PageInfoSchema = Schema.Struct({
  page: Schema.Number,
  pages: Schema.Number,
  results: Schema.Number,
});

const IssuePageSchema = Schema.Struct({
  pageInfo: PageInfoSchema,
  results: Schema.Array(IssueSchema),
});

export type ObserverIssue = Schema.Schema.Type<typeof IssueSchema>;
export type ObserverIssueComment = Schema.Schema.Type<typeof IssueCommentSchema>;
export type ObserverIssueFilter = "all" | "open" | "resolved";

export interface ObserverClientConfig {
  readonly url: string;
  readonly apiKey: string;
  readonly fetchImpl?: typeof fetch;
}

export interface ListIssuesOptions {
  readonly filter?: ObserverIssueFilter;
  readonly maxPages?: number;
  readonly maxRecords?: number;
}

function error(operation: string, cause: unknown): ObserverClientError {
  return cause instanceof ObserverClientError
    ? cause
    : new ObserverClientError({ operation, cause });
}

function decode<A, I>(
  operation: string,
  schema: Schema.Codec<A, I>,
  text: string,
): Effect.Effect<A, ObserverClientError> {
  return Effect.try({
    try: (): A => Schema.decodeUnknownSync(schema)(JSON.parse(text)),
    catch: (cause) => error(operation, cause),
  });
}

export class ObserverClient {
  private readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;

  public constructor(config: ObserverClientConfig) {
    this.baseUrl = config.url.replace(/\/$/, "");
    this.fetchImpl = config.fetchImpl ?? fetch;
    this.apiKey = config.apiKey;
  }

  private readonly apiKey: string;

  private request<A, I>(
    method: string,
    path: string,
    schema: Schema.Codec<A, I>,
    body?: unknown,
  ): Effect.Effect<A, ObserverClientError> {
    return Effect.gen({ self: this }, function* () {
      const operation = `Observer ${method} ${path}`;
      const response = yield* Effect.tryPromise({
        try: (signal) =>
          this.fetchImpl(`${this.baseUrl}/api/v1${path}`, {
            method,
            signal,
            headers: {
              Accept: "application/json",
              "Content-Type": "application/json",
              "X-Api-Key": this.apiKey,
              "User-Agent": "OpenAI File Downloader, XaiImageApiFetch/1.0",
            },
            body: body === undefined ? undefined : JSON.stringify(body),
          }),
        catch: (cause) => error(operation, cause),
      });
      const text = yield* Effect.tryPromise({
        try: (signal) =>
          readFetchResponseTextWithLimit(response, MAX_RESPONSE_BYTES, signal),
        catch: (cause) => error(`${operation} read response`, cause),
      });
      if (!response.ok) {
        return yield* Effect.fail(
          error(operation, new Error(`HTTP ${response.status}`)),
        );
      }
      return yield* decode(operation, schema, text);
    }).pipe(
      Effect.timeout("20 seconds"),
      Effect.mapError((cause) => error(`Observer ${method} ${path}`, cause)),
    );
  }

  public getIssue(issueId: number): Effect.Effect<ObserverIssue, ObserverClientError> {
    return this.request("GET", `/issue/${issueId}`, IssueSchema);
  }

  public listIssues(
    options: ListIssuesOptions = {},
  ): Effect.Effect<readonly ObserverIssue[], ObserverClientError> {
    const filter = options.filter ?? "open";
    const maxPages = Math.min(Math.max(options.maxPages ?? MAX_PAGES, 1), MAX_PAGES);
    const maxRecords = Math.min(
      Math.max(options.maxRecords ?? MAX_RECORDS, 1),
      MAX_RECORDS,
    );
    return Effect.gen({ self: this }, function* () {
      const issues: ObserverIssue[] = [];
      for (let page = 0; page < maxPages && issues.length < maxRecords; page += 1) {
        const response = yield* this.request(
          "GET",
          `/issue?take=${Math.min(PAGE_SIZE, maxRecords - issues.length)}&skip=${issues.length}&filter=${filter}`,
          IssuePageSchema,
        );
        issues.push(...response.results.slice(0, maxRecords - issues.length));
        if (
          response.pageInfo.page >= response.pageInfo.pages ||
          response.results.length === 0
        )
          break;
      }
      return issues;
    });
  }

  public addComment(
    issueId: number,
    message: string,
  ): Effect.Effect<ObserverIssue, ObserverClientError> {
    return this.request("POST", `/issue/${issueId}/comment`, IssueSchema, { message });
  }

  public updateStatus(
    issueId: number,
    status: "open" | "resolved",
  ): Effect.Effect<ObserverIssue, ObserverClientError> {
    return this.request("POST", `/issue/${issueId}/${status}`, IssueSchema);
  }

  public resolveIssue(
    issueId: number,
  ): Effect.Effect<ObserverIssue, ObserverClientError> {
    return this.updateStatus(issueId, "resolved");
  }
}
