import { Effect, Schema } from "effect";
import { readFetchResponseTextWithLimit } from "../effect/publicHttp.js";
import {
  type ArrClient,
  type ArrKind,
  ArrRecoveryError,
  type Grab,
  type ImportFile,
  type QueueItem,
  type Target,
} from "./types.js";

const USER_AGENT = "OpenAI File Downloader, XaiImageApiFetch/1.0";
const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;
const PAGE_SIZE = 250;
const MAX_PAGES = 20;
const MAX_RECORDS = PAGE_SIZE * MAX_PAGES;

const OptionalString = Schema.optional(Schema.NullOr(Schema.String));
const OptionalNumber = Schema.optional(Schema.NullOr(Schema.Number));
const StringOrNumber = Schema.Union([Schema.String, Schema.Number]);
const OptionalStringOrNumber = Schema.optional(Schema.NullOr(StringOrNumber));

const StatusMessageSchema = Schema.Struct({
  title: Schema.String,
  messages: Schema.Array(Schema.String),
});

const QueueRecordSchema = Schema.Struct({
  id: Schema.Number,
  downloadId: OptionalString,
  title: Schema.String,
  status: StringOrNumber,
  trackedDownloadStatus: OptionalStringOrNumber,
  trackedDownloadState: OptionalStringOrNumber,
  statusMessages: Schema.optional(Schema.NullOr(Schema.Array(StatusMessageSchema))),
  size: Schema.Number,
  sizeleft: Schema.optional(Schema.Number),
  sizeLeft: Schema.optional(Schema.Number),
  outputPath: OptionalString,
  added: OptionalString,
  seriesId: OptionalNumber,
  episodeId: OptionalNumber,
  movieId: OptionalNumber,
  protocol: OptionalStringOrNumber,
  downloadClient: OptionalString,
});

const RejectionSchema = Schema.Struct({
  reason: Schema.String,
  type: StringOrNumber,
});

const LanguageSchema = Schema.Struct({
  id: Schema.Number,
  name: Schema.String,
});

const QualityRecordSchema = Schema.Record(Schema.String, Schema.Unknown);
const QualityCoreSchema = Schema.Struct({
  quality: Schema.Struct({
    id: Schema.Number,
    name: Schema.String,
  }),
  revision: Schema.Struct({
    version: Schema.Number,
    real: Schema.Number,
  }),
});

const PreviewRecordSchema = Schema.Struct({
  id: Schema.Number,
  path: Schema.String,
  folderName: OptionalString,
  name: Schema.String,
  size: Schema.Number,
  series: Schema.optional(Schema.NullOr(Schema.Struct({ id: Schema.Number }))),
  movie: Schema.optional(Schema.NullOr(Schema.Struct({ id: Schema.Number }))),
  seasonNumber: OptionalNumber,
  episodes: Schema.optional(
    Schema.NullOr(Schema.Array(Schema.Struct({ id: Schema.Number }))),
  ),
  quality: QualityRecordSchema,
  languages: Schema.optional(Schema.NullOr(Schema.Array(LanguageSchema))),
  releaseGroup: OptionalString,
  indexerFlags: OptionalNumber,
  releaseType: OptionalStringOrNumber,
  rejections: Schema.optional(Schema.NullOr(Schema.Array(RejectionSchema))),
});

const AlternateTitleSchema = Schema.Struct({ title: Schema.String });
const SeriesSchema = Schema.Struct({
  id: Schema.Number,
  title: Schema.String,
  year: Schema.Number,
  monitored: Schema.Boolean,
  path: Schema.String,
  alternateTitles: Schema.optional(Schema.NullOr(Schema.Array(AlternateTitleSchema))),
});

const EpisodeSchema = Schema.Struct({
  id: Schema.Number,
  seasonNumber: Schema.Number,
  episodeNumber: Schema.Number,
  title: Schema.String,
  hasFile: Schema.Boolean,
  monitored: Schema.Boolean,
  episodeFileId: Schema.optional(Schema.Number),
});

const MovieSchema = Schema.Struct({
  id: Schema.Number,
  title: Schema.String,
  year: Schema.Number,
  monitored: Schema.Boolean,
  path: Schema.String,
  hasFile: Schema.optional(Schema.NullOr(Schema.Boolean)),
  movieFileId: Schema.optional(Schema.Number),
  alternateTitles: Schema.optional(Schema.NullOr(Schema.Array(AlternateTitleSchema))),
});

const HistoryRecordSchema = Schema.Struct({
  downloadId: Schema.String,
  sourceTitle: Schema.String,
  seriesId: OptionalNumber,
  movieId: OptionalNumber,
  episodeId: OptionalNumber,
  eventType: StringOrNumber,
  date: Schema.String,
});

const CommandSchema = Schema.Struct({
  id: Schema.Number,
  status: StringOrNumber,
  message: OptionalString,
});

const EpisodeFileSchema = Schema.Struct({
  id: Schema.Number,
  size: Schema.optional(Schema.Number),
  path: Schema.String,
  relativePath: Schema.String,
  sceneName: OptionalString,
});

const MovieFileSchema = Schema.Struct({
  id: Schema.Number,
  size: Schema.optional(Schema.Number),
  movieId: Schema.Number,
  path: Schema.String,
  relativePath: Schema.String,
  sceneName: OptionalString,
  originalFilePath: OptionalString,
});

const FileSystemEntitySchema = Schema.Struct({ path: Schema.String });
const FileSystemSchema = Schema.Struct({
  directories: Schema.Array(FileSystemEntitySchema),
  files: Schema.Array(FileSystemEntitySchema),
});

interface Page<A> {
  readonly page: number;
  readonly pageSize: number;
  readonly totalRecords: number;
  readonly records: readonly A[];
}

interface RawPreview extends Schema.Schema.Type<typeof PreviewRecordSchema> {}

export interface ArrClientConfig {
  readonly kind: ArrKind;
  readonly url: string;
  readonly apiKey: string;
  readonly fetchImpl?: typeof fetch;
}

function operationError(operation: string, cause: unknown): ArrRecoveryError {
  return cause instanceof ArrRecoveryError
    ? cause
    : new ArrRecoveryError({ operation, cause });
}

function valueString(value: string | number | null | undefined): string {
  return value === null || value === undefined ? "" : String(value);
}

function normalizedPath(value: string): string {
  return value.replaceAll("\\", "/").replace(/\/+$/, "").toLowerCase();
}

function baseNameWithoutExtension(value: string): string {
  const name = normalizedPath(value).split("/").at(-1) ?? "";
  return name.replace(
    /\.(?:3g2|3gp|asf|avi|flv|m2ts|m4v|mkv|mov|mp4|mpeg|mpg|mts|ogm|ogv|ts|webm|wmv)$/i,
    "",
  );
}

function sourceMatchesFile(
  source: ImportFile,
  imported: {
    readonly path: string;
    readonly relativePath: string;
    readonly sceneName?: string | null;
    readonly originalFilePath?: string | null;
    readonly size?: number;
  },
): boolean {
  const sourcePath = normalizedPath(source.path);
  const sourceName = baseNameWithoutExtension(source.name || source.path);
  const importedNames = [
    imported.sceneName,
    imported.path,
    imported.relativePath,
    imported.originalFilePath,
  ]
    .filter((value): value is string => Boolean(value))
    .map(baseNameWithoutExtension);

  return (
    normalizedPath(imported.path) === sourcePath ||
    normalizedPath(imported.originalFilePath ?? "") === sourcePath ||
    importedNames.includes(sourceName) ||
    // For obfuscated files Arr records the release folder as sceneName. Require
    // the exact byte size as well as the already-verified target file link.
    (source.folderName !== undefined &&
      imported.sceneName !== undefined &&
      imported.sceneName !== null &&
      source.size > 0 &&
      imported.size === source.size &&
      normalizedPath(imported.sceneName) === normalizedPath(source.folderName))
  );
}

function pageSchema<A, I>(record: Schema.Codec<A, I>) {
  return Schema.Struct({
    page: Schema.Number,
    pageSize: Schema.Number,
    totalRecords: Schema.Number,
    records: Schema.Array(record),
  });
}

export class HttpArrClient implements ArrClient {
  public readonly kind: ArrKind;
  private readonly baseUrl: URL;
  private readonly apiKey: string;
  private readonly fetchImpl: typeof fetch;

  public constructor(config: ArrClientConfig) {
    this.kind = config.kind;
    this.apiKey = config.apiKey;
    this.fetchImpl = config.fetchImpl ?? fetch;
    this.baseUrl = new URL("api/v3/", `${config.url.replace(/\/+$/, "")}/`);
  }

  public queue(): Effect.Effect<QueueItem[], ArrRecoveryError> {
    return this.readPages("read queue", "queue", QueueRecordSchema).pipe(
      Effect.map((records) =>
        records
          .filter((record): record is typeof record & { downloadId: string } =>
            Boolean(record.downloadId),
          )
          .map((record) => ({
            id: record.id,
            downloadId: record.downloadId,
            title: record.title,
            status: valueString(record.status),
            trackedDownloadStatus: valueString(record.trackedDownloadStatus),
            trackedDownloadState: valueString(record.trackedDownloadState),
            statusMessages: (record.statusMessages ?? []).map((message) => ({
              title: message.title,
              messages: [...message.messages],
            })),
            size: record.size,
            sizeleft: record.sizeleft ?? record.sizeLeft ?? 0,
            outputPath: record.outputPath ?? undefined,
            added: record.added ?? undefined,
            seriesId: record.seriesId ?? undefined,
            episodeId: record.episodeId ?? undefined,
            movieId: record.movieId ?? undefined,
            protocol: valueString(record.protocol) || undefined,
            downloadClient: record.downloadClient ?? undefined,
          })),
      ),
    );
  }

  public preview(downloadId: string): Effect.Effect<ImportFile[], ArrRecoveryError> {
    const query = new URLSearchParams({ downloadId });

    return this.request(
      "preview manual import",
      `manualimport?${query}`,
      Schema.Array(PreviewRecordSchema),
    ).pipe(
      Effect.flatMap((records) =>
        Effect.forEach(records, (record) => this.mapPreview(record), {
          concurrency: 4,
        }),
      ),
    );
  }

  public target(items: QueueItem[]): Effect.Effect<Target, ArrRecoveryError> {
    const operation = "read target";
    if (items.length === 0) {
      return Effect.fail(operationError(operation, new Error("queue group is empty")));
    }

    if (this.kind === "radarr") {
      const ids = new Set(items.map((item) => item.movieId).filter(Number.isInteger));
      if (ids.size !== 1) {
        return Effect.fail(
          operationError(operation, new Error("queue group must identify one movie")),
        );
      }
      const id = [...ids][0] as number;
      return this.request(operation, `movie/${id}`, MovieSchema).pipe(
        Effect.map((movie) => ({
          id: movie.id,
          title: movie.title,
          year: movie.year,
          monitored: movie.monitored,
          hasFile: movie.hasFile ?? (movie.movieFileId ?? 0) > 0,
          path: movie.path,
          episodeIds: [],
          episodes: [],
          alternateTitles: (movie.alternateTitles ?? []).map((item) => item.title),
        })),
      );
    }

    const seriesIds = new Set(
      items.map((item) => item.seriesId).filter(Number.isInteger),
    );
    const episodeIds = [
      ...new Set(items.map((item) => item.episodeId).filter(Number.isInteger)),
    ] as number[];
    if (seriesIds.size !== 1 || episodeIds.length === 0) {
      return Effect.fail(
        operationError(
          operation,
          new Error("queue group must identify one series and its episodes"),
        ),
      );
    }
    const id = [...seriesIds][0] as number;
    return Effect.all(
      [
        this.request(operation, `series/${id}`, SeriesSchema),
        this.request(operation, `episode?seriesId=${id}`, Schema.Array(EpisodeSchema)),
      ],
      { concurrency: 2 },
    ).pipe(
      Effect.flatMap(([series, allEpisodes]) => {
        const wanted = new Set(episodeIds);
        const episodes = allEpisodes.filter((episode) => wanted.has(episode.id));
        if (episodes.length !== wanted.size) {
          return Effect.fail(
            operationError(
              operation,
              new Error("target episode metadata is incomplete"),
            ),
          );
        }
        return Effect.succeed({
          id: series.id,
          title: series.title,
          year: series.year,
          monitored: series.monitored,
          hasFile: episodes.every((episode) => episode.hasFile),
          path: series.path,
          episodeIds,
          episodes: episodes.map((episode) => ({
            id: episode.id,
            seasonNumber: episode.seasonNumber,
            episodeNumber: episode.episodeNumber,
            title: episode.title,
            hasFile: episode.hasFile,
            monitored: episode.monitored,
          })),
          alternateTitles: (series.alternateTitles ?? []).map((item) => item.title),
        });
      }),
    );
  }

  public history(downloadId: string): Effect.Effect<Grab[], ArrRecoveryError> {
    const query = new URLSearchParams({ downloadId, eventType: "1" });
    return this.readPages(
      "read grab history",
      `history?${query}`,
      HistoryRecordSchema,
    ).pipe(
      Effect.map((records) =>
        records
          .filter((record) => record.downloadId === downloadId)
          .map((record) => ({
            downloadId: record.downloadId,
            sourceTitle: record.sourceTitle,
            seriesId: record.seriesId ?? undefined,
            movieId: record.movieId ?? undefined,
            episodeId: record.episodeId ?? undefined,
            eventType: valueString(record.eventType),
            date: record.date,
          })),
      ),
    );
  }

  public importFiles(
    downloadId: string,
    files: ImportFile[],
  ): Effect.Effect<number, ArrRecoveryError> {
    const bodyFiles = files.map((file) => ({
      path: file.path,
      folderName: file.folderName,
      seriesId: file.seriesId,
      movieId: file.movieId,
      episodeIds: this.kind === "sonarr" ? file.episodeIds : undefined,
      quality: file.quality,
      languages: file.languages ?? [],
      releaseGroup: file.releaseGroup ?? "",
      indexerFlags: file.indexerFlags ?? 0,
      releaseType: this.kind === "sonarr" ? (file.releaseType ?? "unknown") : undefined,
      downloadId,
    }));

    return this.request("start manual import", "command", CommandSchema, {
      method: "POST",
      body: JSON.stringify({
        name: "ManualImport",
        importMode: "auto",
        files: bodyFiles,
      }),
    }).pipe(Effect.map((command) => command.id));
  }

  public command(
    id: number,
  ): Effect.Effect<{ status: string; message?: string }, ArrRecoveryError> {
    return this.request("read command", `command/${id}`, CommandSchema).pipe(
      Effect.map((command) => ({
        status: valueString(command.status),
        message: command.message ?? undefined,
      })),
    );
  }

  public remove(id: number, blocklist: boolean): Effect.Effect<void, ArrRecoveryError> {
    const query = new URLSearchParams({
      removeFromClient: "true",
      blocklist: String(blocklist),
      skipRedownload: "true",
    });
    return this.requestVoid("remove queue item", `queue/${id}?${query}`, {
      method: "DELETE",
    });
  }

  public verifyRemoved(outputPath: string): Effect.Effect<boolean, ArrRecoveryError> {
    const normalized = normalizedPath(outputPath);
    const slash = normalized.lastIndexOf("/");
    if (!normalized || slash <= 0) {
      return Effect.fail(
        operationError("verify removed data", new Error("invalid output path")),
      );
    }
    const parent = outputPath.replaceAll("\\", "/").replace(/\/+$/, "").slice(0, slash);
    const query = new URLSearchParams({
      path: parent,
      includeFiles: "true",
      allowFoldersWithoutTrailingSlashes: "true",
    });
    return this.request(
      "verify removed data",
      `filesystem?${query}`,
      FileSystemSchema,
    ).pipe(
      Effect.map((contents) => {
        const entities = [...contents.directories, ...contents.files];
        if (entities.some((entity) => normalizedPath(entity.path) === normalized)) {
          return false;
        }
        // Arr deliberately maps missing, inaccessible, and empty directories to
        // the same empty response. Require another entry as positive evidence
        // that the parent directory was readable.
        return entities.length > 0;
      }),
    );
  }

  public search(target: Target): Effect.Effect<number, ArrRecoveryError> {
    const command =
      this.kind === "sonarr"
        ? { name: "EpisodeSearch", episodeIds: target.episodeIds }
        : { name: "MoviesSearch", movieIds: [target.id] };
    return this.request("start replacement search", "command", CommandSchema, {
      method: "POST",
      body: JSON.stringify(command),
    }).pipe(Effect.map((result) => result.id));
  }

  public verifyImported(
    target: Target,
    files: ImportFile[],
  ): Effect.Effect<boolean, ArrRecoveryError> {
    if (files.length === 0) return Effect.succeed(false);
    if (this.kind === "radarr") {
      return Effect.all(
        [
          this.request("verify movie import", `movie/${target.id}`, MovieSchema),
          this.request(
            "verify movie import",
            `moviefile?movieId=${target.id}`,
            Schema.Array(MovieFileSchema),
          ),
        ],
        { concurrency: 2 },
      ).pipe(
        Effect.map(([movie, imported]) => {
          if (!(movie.hasFile ?? (movie.movieFileId ?? 0) > 0)) return false;
          return files.every((source) =>
            imported.some(
              (file) =>
                file.id === movie.movieFileId &&
                file.movieId === target.id &&
                sourceMatchesFile(source, file),
            ),
          );
        }),
      );
    }

    return Effect.all(
      [
        this.request(
          "verify episode import",
          `episode?seriesId=${target.id}`,
          Schema.Array(EpisodeSchema),
        ),
        this.request(
          "verify episode import",
          `episodefile?seriesId=${target.id}`,
          Schema.Array(EpisodeFileSchema),
        ),
      ],
      { concurrency: 2 },
    ).pipe(
      Effect.map(([episodes, imported]) => {
        const wanted = new Set(target.episodeIds);
        const targeted = episodes.filter((episode) => wanted.has(episode.id));
        if (
          targeted.length !== wanted.size ||
          targeted.some((episode) => !episode.hasFile || !(episode.episodeFileId ?? 0))
        ) {
          return false;
        }
        const importedIds = new Set(imported.map((file) => file.id));
        if (
          targeted.some((episode) => !importedIds.has(episode.episodeFileId as number))
        ) {
          return false;
        }
        const targetedById = new Map(targeted.map((episode) => [episode.id, episode]));
        const coveredEpisodeIds = new Set(files.flatMap((file) => file.episodeIds));
        if (targeted.some((episode) => !coveredEpisodeIds.has(episode.id)))
          return false;

        return files.every((source) => {
          if (source.episodeIds.length === 0) return false;
          const matchingFileIds = new Set(
            imported
              .filter((file) => sourceMatchesFile(source, file))
              .map((file) => file.id),
          );
          return source.episodeIds.every((episodeId) => {
            const episode = targetedById.get(episodeId);
            return (
              episode !== undefined &&
              matchingFileIds.has(episode.episodeFileId as number)
            );
          });
        });
      }),
    );
  }

  private mapPreview(raw: RawPreview): Effect.Effect<ImportFile, ArrRecoveryError> {
    return Schema.decodeUnknownEffect(QualityCoreSchema)(raw.quality).pipe(
      Effect.mapError((cause) => operationError("decode manual import quality", cause)),
      Effect.map(() => ({
        id: raw.id,
        path: raw.path,
        name: raw.name,
        size: raw.size,
        seriesId: raw.series?.id,
        movieId: raw.movie?.id,
        seasonNumber: raw.seasonNumber ?? undefined,
        episodeIds: (raw.episodes ?? []).map((episode) => episode.id),
        quality: { ...raw.quality },
        languages: raw.languages ? [...raw.languages] : undefined,
        releaseGroup: raw.releaseGroup ?? undefined,
        indexerFlags: raw.indexerFlags ?? undefined,
        releaseType: valueString(raw.releaseType) || undefined,
        rejections: (raw.rejections ?? []).map((rejection) => ({
          reason: rejection.reason,
          type: valueString(rejection.type),
        })),
        ...(raw.folderName ? { folderName: raw.folderName } : {}),
      })),
    );
  }

  private readPages<A, I>(
    operation: string,
    path: string,
    recordSchema: Schema.Codec<A, I>,
  ): Effect.Effect<A[], ArrRecoveryError> {
    const requestPage = (pagePath: string) =>
      this.request(operation, pagePath, pageSchema(recordSchema));
    return Effect.gen(function* () {
      const records: A[] = [];
      for (let page = 1; page <= MAX_PAGES; page += 1) {
        const separator = path.includes("?") ? "&" : "?";
        const result: Page<A> = yield* requestPage(
          `${path}${separator}page=${page}&pageSize=${PAGE_SIZE}`,
        );
        records.push(...result.records);
        if (records.length > MAX_RECORDS) {
          return yield* Effect.fail(
            operationError(operation, new Error("record limit exceeded")),
          );
        }
        if (
          result.records.length === 0 ||
          records.length >= result.totalRecords ||
          result.records.length < result.pageSize
        ) {
          return records;
        }
      }
      return yield* Effect.fail(
        operationError(operation, new Error("page limit exceeded")),
      );
    });
  }

  private request<A, I>(
    operation: string,
    path: string,
    schema: Schema.Codec<A, I>,
    init: RequestInit = {},
  ): Effect.Effect<A, ArrRecoveryError> {
    const url = new URL(path, this.baseUrl);
    const fetchImpl = this.fetchImpl;
    const apiKey = this.apiKey;
    const request = Effect.gen(function* () {
      const response = yield* Effect.tryPromise({
        try: (signal) =>
          fetchImpl(url, {
            ...init,
            headers: {
              Accept: "application/json",
              "Content-Type": "application/json",
              "User-Agent": USER_AGENT,
              "X-Api-Key": apiKey,
              ...init.headers,
            },
            signal,
          }),
        catch: (cause) => operationError(operation, cause),
      });
      if (!response.ok) {
        return yield* Effect.fail(
          operationError(operation, new Error(`HTTP ${response.status}`)),
        );
      }
      const text = yield* Effect.tryPromise({
        try: (signal) =>
          readFetchResponseTextWithLimit(response, MAX_RESPONSE_BYTES, signal),
        catch: (cause) => operationError(operation, cause),
      });
      const json = yield* Effect.try({
        try: () => JSON.parse(text) as unknown,
        catch: (cause) => operationError(operation, cause),
      });
      return yield* Schema.decodeUnknownEffect(schema)(json).pipe(
        Effect.mapError((cause) => operationError(operation, cause)),
      );
    });
    return request.pipe(
      Effect.timeout("15 seconds"),
      Effect.mapError((cause) => operationError(operation, cause)),
    );
  }

  private requestVoid(
    operation: string,
    path: string,
    init: RequestInit,
  ): Effect.Effect<void, ArrRecoveryError> {
    const url = new URL(path, this.baseUrl);
    return Effect.tryPromise({
      try: (signal) =>
        this.fetchImpl(url, {
          ...init,
          headers: {
            Accept: "application/json",
            "User-Agent": USER_AGENT,
            "X-Api-Key": this.apiKey,
            ...init.headers,
          },
          signal,
        }),
      catch: (cause) => operationError(operation, cause),
    }).pipe(
      Effect.timeout("15 seconds"),
      Effect.flatMap((response) =>
        response.ok
          ? Effect.void
          : Effect.fail(
              operationError(operation, new Error(`HTTP ${response.status}`)),
            ),
      ),
      Effect.mapError((cause) => operationError(operation, cause)),
    );
  }
}

export function createArrClient(config: ArrClientConfig): ArrClient {
  return new HttpArrClient(config);
}
