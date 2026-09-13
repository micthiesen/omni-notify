import { Data, Effect, Schema } from "effect";
import { readFetchResponseTextWithLimit } from "../effect/publicHttp.js";

const USER_AGENT = "OpenAI File Downloader, XaiImageApiFetch/1.0";
const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;
export type ArrRepairKind = "sonarr" | "radarr";
export interface ArrRepairMedia {
  readonly tmdbId?: number | null;
  readonly tvdbId?: number | null;
  readonly mediaType?: string | null;
}
export class ArrRepairError extends Data.TaggedError("ArrRepairError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    return `${this.operation}: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}
const Id = Schema.Number.pipe(Schema.check(Schema.isInt(), Schema.isGreaterThan(0)));
const CommandSchema = Schema.Struct({ id: Id });
const MediaSchema = Schema.Struct({
  id: Id,
  title: Schema.String,
  monitored: Schema.Boolean,
  tmdbId: Schema.optional(Schema.Number),
  tvdbId: Schema.optional(Schema.Number),
});
const EpisodeSchema = Schema.Struct({
  id: Id,
  seriesId: Id,
  seasonNumber: Schema.Number,
  episodeNumber: Schema.Number,
  hasFile: Schema.Boolean,
  monitored: Schema.Boolean,
  episodeFileId: Schema.Number,
});
const FileSchema = Schema.Struct({
  id: Id,
  seriesId: Schema.optional(Id),
  movieId: Schema.optional(Id),
  path: Schema.String,
  sceneName: Schema.optional(Schema.NullOr(Schema.String)),
});
const HistorySchema = Schema.Struct({
  id: Id,
  seriesId: Schema.optional(Id),
  movieId: Schema.optional(Id),
  episodeId: Schema.optional(Id),
  sourceTitle: Schema.String,
  downloadId: Schema.optional(Schema.NullOr(Schema.String)),
  eventType: Schema.Union([Schema.String, Schema.Number]),
  data: Schema.Struct({
    fileId: Schema.optional(Schema.String),
    importedPath: Schema.optional(Schema.String),
  }),
});
const QueueSchema = Schema.Struct({
  id: Id,
  seriesId: Schema.optional(Id),
  movieId: Schema.optional(Id),
  episodeId: Schema.optional(Id),
  downloadId: Schema.optional(Schema.String),
});
export interface ArrInspection {
  readonly kind: ArrRepairKind;
  readonly id: number;
  readonly title: string;
  readonly monitored: boolean;
  readonly episodes: readonly (typeof EpisodeSchema.Type)[];
  readonly files: readonly (typeof FileSchema.Type)[];
  readonly history: readonly (typeof HistorySchema.Type)[];
  readonly queue: readonly (typeof QueueSchema.Type)[];
}
export interface ArrRepairScope {
  readonly kind: ArrRepairKind;
  readonly id: number;
  readonly title: string;
  readonly description: string;
  readonly episodeIds: readonly number[];
  readonly fileIds: readonly number[];
  readonly historyIds: readonly number[];
  readonly releases: readonly string[];
}
export type ArrRepairInstruction = {
  readonly action: "replace" | "search_missing";
  readonly season: number | null;
  readonly episodes: readonly number[];
};
export interface ArrRepairClientConfig {
  readonly kind: ArrRepairKind;
  readonly url: string;
  readonly apiKey: string;
  readonly fetchImpl?: typeof fetch;
}
function event(row: typeof HistorySchema.Type, name: string, number: number) {
  return row.eventType === name || row.eventType === number;
}

/** Resolves identifiers from Arr, never from model output or display-name matching. */
export class ArrRepairClient {
  private readonly baseUrl: URL;
  private readonly fetchImpl: typeof fetch;
  private readonly apiKey: string;
  public constructor(private readonly config: ArrRepairClientConfig) {
    this.baseUrl = new URL("api/v3/", `${config.url.replace(/\/+$/, "")}/`);
    this.fetchImpl = config.fetchImpl ?? fetch;
    this.apiKey = config.apiKey;
  }
  public inspect(media: ArrRepairMedia): Effect.Effect<ArrInspection, ArrRepairError> {
    return Effect.gen({ self: this }, function* () {
      const movie = this.config.kind === "radarr";
      const externalId = movie ? media.tmdbId : media.tvdbId;
      if (media.mediaType !== (movie ? "movie" : "tv") || !externalId)
        return yield* this.error(
          "identify title",
          "Missing matching TMDB/TVDB identity",
        );
      const key = movie ? "tmdbId" : "tvdbId";
      const matches = (yield* this.request(
        "identify title",
        `${movie ? "movie" : "series"}?${key}=${externalId}`,
        Schema.Array(MediaSchema),
      )).filter((item) => item[key] === externalId);
      if (matches.length !== 1)
        return yield* this.error(
          "identify title",
          "Exact title is missing or ambiguous",
        );
      const target = matches[0]!;
      const relation = movie ? "movieId" : "seriesId";
      const episodes = movie
        ? []
        : yield* this.request(
            "inspect episodes",
            `episode?seriesId=${target.id}`,
            Schema.Array(EpisodeSchema),
          );
      const files = yield* this.request(
        "inspect files",
        `${movie ? "moviefile" : "episodefile"}?${relation}=${target.id}`,
        Schema.Array(FileSchema),
      );
      const history = yield* this.request(
        "inspect history",
        `history/${movie ? "movie" : "series"}?${relation}=${target.id}`,
        Schema.Array(HistorySchema),
      );
      const queue = yield* this.queue();
      if (episodes.length > 2000 || history.length > 10000 || files.length > 2000)
        return yield* this.error(
          "inspect title",
          "Title exceeds bounded repair inspection",
        );
      return {
        kind: this.config.kind,
        id: target.id,
        title: target.title,
        monitored: target.monitored,
        episodes: episodes.filter((e) => e.seriesId === target.id),
        files: files.filter((f) => f[relation] === target.id),
        history: history.filter((h) => h[relation] === target.id),
        queue: queue.filter((q) => q[relation] === target.id),
      };
    });
  }
  private queue() {
    return this.request(
      "inspect queue",
      "queue?page=1&pageSize=1000",
      Schema.Struct({
        totalRecords: Schema.Number,
        records: Schema.Array(QueueSchema),
      }),
    ).pipe(
      Effect.flatMap((page) =>
        page.totalRecords > page.records.length
          ? Effect.fail(this.error("inspect queue", "Queue exceeds inspection bound"))
          : Effect.succeed(page.records),
      ),
    );
  }
  public plan(
    inspected: ArrInspection,
    instruction: ArrRepairInstruction,
  ): Effect.Effect<ArrRepairScope, ArrRepairError> {
    return Effect.try({
      try: () => {
        const fail = (message: string): never => {
          throw new Error(message);
        };
        if (inspected.kind !== this.config.kind || !inspected.monitored)
          fail("Unmonitored or mismatched title");
        const wanted = inspected.episodes.filter(
          (e) =>
            (instruction.season === null
              ? e.seasonNumber > 0
              : e.seasonNumber === instruction.season) &&
            (!instruction.episodes.length ||
              instruction.episodes.includes(e.episodeNumber)),
        );
        if (inspected.kind === "sonarr") {
          if (!wanted.length) fail("Requested episode scope is empty");
          if (
            instruction.episodes.some((n) => !wanted.some((e) => e.episodeNumber === n))
          )
            fail("One or more requested episodes do not exist");
        }
        const targets =
          instruction.action === "search_missing"
            ? wanted.filter((e) => !e.hasFile)
            : wanted;
        if (targets.some((e) => !e.monitored))
          fail("Requested scope includes unmonitored episodes");
        if (inspected.kind === "sonarr" && !targets.length)
          fail("Requested episodes are already present");
        if (
          instruction.action === "search_missing" &&
          inspected.kind === "radarr" &&
          inspected.files.length
        )
          fail("Movie is already present");
        const episodeIds = targets.map((e) => e.id);
        // Queue removal can affect a complete season pack. Leave active downloads to Arr recovery.
        if (
          inspected.queue.some(
            (q) =>
              inspected.kind === "radarr" ||
              !q.episodeId ||
              episodeIds.includes(q.episodeId),
          )
        )
          fail("An active download overlaps the requested scope");
        const fileIds =
          instruction.action === "search_missing"
            ? []
            : inspected.kind === "radarr"
              ? inspected.files.map((f) => f.id)
              : [
                  ...new Set(
                    targets.filter((e) => e.hasFile).map((e) => e.episodeFileId),
                  ),
                ];
        const historyIds: number[] = [];
        const releases: string[] = [];
        for (const fileId of fileIds) {
          const file = inspected.files.find((f) => f.id === fileId);
          if (!file) fail("Current file mapping is incomplete");
          if (
            inspected.episodes.some(
              (e) => e.episodeFileId === fileId && !episodeIds.includes(e.id),
            )
          )
            fail("File contains episodes outside the requested scope");
          const imports = inspected.history.filter(
            (h) =>
              event(h, "downloadFolderImported", 3) &&
              (h.data.fileId === String(fileId) ||
                (h.data.importedPath === file!.path &&
                  h.sourceTitle === file!.sceneName)) &&
              h.downloadId,
          );
          const downloads = [...new Set(imports.map((h) => h.downloadId!))];
          if (downloads.length !== 1)
            fail("Current file lacks unambiguous import history");
          const grabs = inspected.history.filter(
            (h) => event(h, "grabbed", 1) && h.downloadId === downloads[0],
          );
          if (!grabs.length)
            fail("Current file lacks matching grabbed release history");
          if (
            inspected.kind === "sonarr" &&
            grabs.some((h) => !h.episodeId || !episodeIds.includes(h.episodeId))
          )
            fail(
              "Blocklisting the release would also re-search episodes outside scope",
            );
          const grab = grabs[0]!;
          if (!releases.includes(grab.sourceTitle)) {
            historyIds.push(grab.id);
            releases.push(grab.sourceTitle);
          }
        }
        const scopeText =
          inspected.kind === "radarr"
            ? "movie"
            : instruction.season === null
              ? "series"
              : `season ${instruction.season}${instruction.episodes.length ? ` episodes ${instruction.episodes.join(", ")}` : ""}`;
        return {
          kind: inspected.kind,
          id: inspected.id,
          title: inspected.title,
          description: `${inspected.title}: ${scopeText}. ${instruction.action === "replace" ? `Blocklisted ${historyIds.length} release(s) and removed ${fileIds.length} file(s).` : "Searched missing files only."}`,
          episodeIds,
          fileIds,
          historyIds,
          releases,
        };
      },
      catch: (cause) => this.error("plan repair", cause),
    });
  }
  public blocklistAndDelete(
    scope: ArrRepairScope,
  ): Effect.Effect<void, ArrRepairError> {
    return Effect.gen({ self: this }, function* () {
      for (const id of scope.historyIds)
        yield* this.requestVoid("blocklist release", `history/failed/${id}`, {
          method: "POST",
        });
      if (scope.releases.length) {
        const blocklist = yield* this.request(
          "verify blocklist",
          "blocklist?page=1&pageSize=1000&sortKey=date&sortDirection=descending",
          Schema.Struct({
            records: Schema.Array(
              Schema.Struct({
                sourceTitle: Schema.String,
                seriesId: Schema.optional(Id),
                movieId: Schema.optional(Id),
              }),
            ),
          }),
        );
        if (
          scope.releases.some(
            (title) =>
              !blocklist.records.some(
                (r) =>
                  r.sourceTitle === title &&
                  (scope.kind === "sonarr" ? r.seriesId : r.movieId) === scope.id,
              ),
          )
        )
          return yield* this.error(
            "verify blocklist",
            "Release not visible in blocklist",
          );
      }
      const resource = scope.kind === "sonarr" ? "episodefile" : "moviefile";
      for (const id of scope.fileIds)
        yield* this.requestVoid("delete media file", `${resource}/${id}`, {
          method: "DELETE",
        });
      const remaining = yield* this.request(
        "verify file deletion",
        `${resource}?${scope.kind === "sonarr" ? "seriesId" : "movieId"}=${scope.id}`,
        Schema.Array(FileSchema),
      );
      if (remaining.some((f) => scope.fileIds.includes(f.id)))
        return yield* this.error(
          "verify file deletion",
          "Deleted file is still present",
        );
    });
  }
  public search(scope: ArrRepairScope): Effect.Effect<number, ArrRepairError> {
    return this.request("start automatic search", "command", CommandSchema, {
      method: "POST",
      body: JSON.stringify(
        scope.kind === "sonarr"
          ? { name: "EpisodeSearch", episodeIds: scope.episodeIds }
          : { name: "MoviesSearch", movieIds: [scope.id] },
      ),
    }).pipe(Effect.map((result) => result.id));
  }
  public replace(scope: ArrRepairScope): Effect.Effect<number, ArrRepairError> {
    return this.blocklistAndDelete(scope).pipe(
      Effect.flatMap(() => this.search(scope)),
    );
  }
  private request<A, I>(
    operation: string,
    path: string,
    schema: Schema.Codec<A, I>,
    init: RequestInit = {},
  ): Effect.Effect<A, ArrRepairError> {
    return Effect.gen({ self: this }, function* () {
      const url = new URL(path, this.baseUrl);
      const response = yield* Effect.tryPromise({
        try: (signal) =>
          this.fetchImpl(url, {
            ...init,
            headers: {
              Accept: "application/json",
              "Content-Type": "application/json",
              "User-Agent": USER_AGENT,
              "X-Api-Key": this.apiKey,
              ...init.headers,
            },
            signal,
          }),
        catch: (cause) => new ArrRepairError({ operation, cause }),
      });
      if (!response.ok)
        return yield* Effect.fail(
          new ArrRepairError({ operation, cause: `HTTP ${response.status}` }),
        );
      const text = yield* Effect.tryPromise({
        try: (signal) =>
          readFetchResponseTextWithLimit(response, MAX_RESPONSE_BYTES, signal),
        catch: (cause) => new ArrRepairError({ operation, cause }),
      });
      const raw = yield* Effect.try({
        try: () => JSON.parse(text) as unknown,
        catch: (cause) => new ArrRepairError({ operation, cause }),
      });
      return yield* Schema.decodeUnknownEffect(schema)(raw).pipe(
        Effect.mapError((cause) => new ArrRepairError({ operation, cause })),
      );
    }).pipe(
      Effect.timeout("15 seconds"),
      Effect.mapError((cause) =>
        cause instanceof ArrRepairError
          ? cause
          : new ArrRepairError({ operation, cause }),
      ),
    );
  }

  private error(operation: string, cause: unknown): ArrRepairError {
    return new ArrRepairError({ operation, cause });
  }

  private requestVoid(
    operation: string,
    path: string,
    init: RequestInit & { searchParams?: Record<string, string> },
  ): Effect.Effect<void, ArrRepairError> {
    const { searchParams, ...requestInit } = init;
    const query = searchParams ? `?${new URLSearchParams(searchParams)}` : "";
    const url = new URL(`${path}${query}`, this.baseUrl);
    return Effect.tryPromise({
      try: (signal) =>
        this.fetchImpl(url, {
          ...requestInit,
          headers: {
            Accept: "application/json",
            "User-Agent": USER_AGENT,
            "X-Api-Key": this.apiKey,
            ...requestInit.headers,
          },
          signal,
        }),
      catch: (cause) => new ArrRepairError({ operation, cause }),
    }).pipe(
      Effect.timeout("15 seconds"),
      Effect.flatMap((response) =>
        response.ok
          ? Effect.void
          : Effect.fail(
              new ArrRepairError({ operation, cause: `HTTP ${response.status}` }),
            ),
      ),
      Effect.mapError((cause) =>
        cause instanceof ArrRepairError
          ? cause
          : new ArrRepairError({ operation, cause }),
      ),
    );
  }
}
