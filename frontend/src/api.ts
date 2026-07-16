export type TaskTrigger = "schedule" | "manual" | "startup" | "catchup";
export type TaskRunStatus = "running" | "success" | "error";

export interface TaskRun {
  runId: string;
  taskName: string;
  trigger: TaskTrigger;
  scheduledFor: number | null;
  startedAt: number;
  finishedAt: number | null;
  status: TaskRunStatus;
  error: string | null;
  summary: string | null;
}

export interface TaskInfo {
  name: string;
  schedule: string;
  running: boolean;
  nextRuns: string[];
  lastRun: TaskRun | null;
}

export interface ManualRunOptions {
  maxRecommendations?: number;
}

export type RunLogLevel = "debug" | "info" | "warn" | "error";

export interface RunLogLine {
  /** Epoch ms of the log call */
  t: number;
  level: RunLogLevel;
  /** Logger name, e.g. "Main:LiveCheck" */
  logger: string;
  msg: string;
}

export interface RunLogs {
  run: TaskRun;
  lines: RunLogLine[];
  /** Oldest lines dropped once the per-run cap was hit. */
  dropped: number;
}

export interface StreamerBinding {
  platform: string;
  username: string;
  url: string;
}

interface StreamerBase {
  id: string;
  displayName: string;
  bindings: StreamerBinding[];
}

export type LiveStreamer = StreamerBase & {
  live: true;
  title: string;
  startedAt: number;
  maxViewerCount: number;
  primary: StreamerBinding;
};

export type OfflineStreamer = StreamerBase & {
  live: false;
  lastStartedAt: number | null;
  lastEndedAt: number | null;
  lastMaxViewerCount: number | null;
};

export type StreamerView = LiveStreamer | OfflineStreamer;

export interface DailyViewerBucket {
  /** YYYY-MM-DD (UTC) */
  date: string;
  maxViewers: number;
  timestamp: number;
}

export interface StreamerMetrics {
  dailyBuckets: DailyViewerBucket[];
  allTimeMax: number;
  allTimeMaxTimestamp: number;
}

export interface Snapshot {
  tasks: TaskInfo[];
  streamers: StreamerView[];
  runs: TaskRun[];
}

export type DataValue =
  | string
  | number
  | boolean
  | null
  | DataValue[]
  | { [key: string]: DataValue };

export type DataRow = Record<string, DataValue>;

export interface DataEntity {
  slug: string;
  label: string;
  description: string;
  warning?: string;
  primaryKey: string[];
  count: number;
  storageBytes: number;
}

export interface DataStorageSummary {
  databaseSizeBytes: number;
  entityStorageBytes: number;
}

export type MediaType = "movie" | "tv";
export type RecommendationStatus =
  | "pending"
  | "notified"
  | "watched"
  | "abandoned"
  | "ignored"
  | "failed";
export type WatchlistResult = "added" | "already_exists" | "available" | "error";
export type RecommendationFeedback =
  | "good_pick"
  | "not_for_me"
  | "already_watched";

export interface Recommendation {
  recommendationId: string;
  canonicalId: string;
  tmdbId: number;
  mediaType: MediaType;
  title: string;
  year: number | null;
  posterPath: string | null;
  status: RecommendationStatus;
  whyForUser: string | null;
  caveats: string[];
  runDate: string;
  recommendedAt: number;
  notifiedAt: number | null;
  startedAt: number | null;
  resolvedAt: number | null;
  watchlistResult: WatchlistResult | null;
  confidence: number | null;
  feedback: RecommendationFeedback | null;
  feedbackAt: number | null;
  source: string | null;
  genres: string[];
  runtimeMinutes: number | null;
  seasonCount: number | null;
  episodeCount: number | null;
  seriesStatus: string | null;
  originalLanguage: string | null;
  originCountries: string[];
  creators: string[];
  cast: string[];
  keywords: string[];
  certification: string | null;
  shortlistScores: {
    tasteMatch: number;
    novelty: number;
    effortFit: number;
    composite: number;
    risks: string[];
  } | null;
  links: { tmdb: string; plex: string; manager: string };
}

export interface TasteClaim {
  claim: string;
  confidence: number;
  evidenceIds: string[];
}

export interface TasteBehaviorStats {
  completedMovies: number;
  completedSeries: number;
  rewatchedTitles: number;
  recommendations: {
    total: number;
    watched: number;
    abandoned: number;
    ignored: number;
    failed: number;
    awaitingOutcome: number;
  };
  feedback: {
    goodPick: number;
    notForMe: number;
    alreadyWatched: number;
  };
  averageHoursToStart?: number;
  sourcePerformance: Record<
    string,
    { total: number; watched: number; goodPick: number; notForMe: number }
  >;
}

export interface TasteProfile {
  profileId: string;
  version: number;
  generatedAt: number;
  summary: string;
  stablePreferences: TasteClaim[];
  conditionalPreferences: TasteClaim[];
  aversions: TasteClaim[];
  currentSaturation: TasteClaim[];
  explorationTargets: TasteClaim[];
  uncertainties: TasteClaim[];
  commitmentPreferences: {
    movies: { preference: string; confidence: number; evidenceIds: string[] };
    limitedSeries: { preference: string; confidence: number; evidenceIds: string[] };
    longSeries: { preference: string; confidence: number; evidenceIds: string[] };
  };
  stats: TasteBehaviorStats;
}

export type PodcastRecommendationStatus =
  | "pending"
  | "notified"
  | "listened"
  | "abandoned"
  | "ignored"
  | "failed";
export type PodcastFeedback = "good_pick" | "not_for_me";
export type PodcastQueueResult = "queued" | "already_queued" | "not_queued";

export interface PodcastRecommendation {
  recommendationId: string;
  showTitle: string;
  episodeTitle: string;
  feedUrl: string;
  itunesId?: number;
  artworkUrl?: string;
  episodeUrl?: string;
  publishedAt: number;
  durationMinutes?: number;
  status: PodcastRecommendationStatus;
  whyForUser?: string;
  caveats?: string[];
  confidence?: number;
  shortlistScores?: {
    tasteMatch: number;
    novelty: number;
    composite: number;
    risks: string[];
  };
  discoveredVia?: string;
  sourceUrl?: string;
  matchedVoices?: string[];
  recommendedAt: number;
  notifiedAt?: number;
  queueResult?: PodcastQueueResult | null;
  feedback?: PodcastFeedback;
  feedbackAt?: number;
}

export class ApiError extends Error {
  public readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

async function extractErrorMessage(res: Response): Promise<string> {
  try {
    const body: unknown = await res.json();
    if (
      typeof body === "object" &&
      body !== null &&
      "error" in body &&
      typeof (body as { error: unknown }).error === "string"
    ) {
      return (body as { error: string }).error;
    }
  } catch {
    // fall through to generic message
  }
  return `HTTP ${res.status}: ${res.statusText}`;
}

export async function apiGet<T>(path: string): Promise<T> {
  const res = await fetch(path);
  if (!res.ok) {
    throw new ApiError(res.status, await extractErrorMessage(res));
  }
  return (await res.json()) as T;
}

export async function apiPost<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method: "POST",
    ...(body === undefined
      ? {}
      : {
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        }),
  });
  if (!res.ok) {
    throw new ApiError(res.status, await extractErrorMessage(res));
  }
  return (await res.json()) as T;
}

export async function apiDelete<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(path, {
    method: "DELETE",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    throw new ApiError(res.status, await extractErrorMessage(res));
  }
  return (await res.json()) as T;
}

export function fetchTasks(): Promise<{ tasks: TaskInfo[] }> {
  return apiGet<{ tasks: TaskInfo[] }>("/api/tasks");
}

export function fetchSnapshot(): Promise<Snapshot> {
  return apiGet<Snapshot>("/api/snapshot");
}

export function fetchDataEntities(): Promise<{
  entities: DataEntity[];
  storage: DataStorageSummary;
}> {
  return apiGet<{ entities: DataEntity[]; storage: DataStorageSummary }>(
    "/api/data/entities",
  );
}

export function fetchDataRows(
  slug: string,
): Promise<{ summary: DataEntity; rows: DataRow[] }> {
  return apiGet<{ summary: DataEntity; rows: DataRow[] }>(
    `/api/data/entities/${encodeURIComponent(slug)}`,
  );
}

export function deleteDataRow(
  slug: string,
  key: DataRow,
): Promise<{ deleted: true }> {
  return apiDelete<{ deleted: true }>(
    `/api/data/entities/${encodeURIComponent(slug)}`,
    { key },
  );
}

export function fetchTaskRuns(options?: {
  task?: string;
  limit?: number;
}): Promise<{ runs: TaskRun[] }> {
  const params = new URLSearchParams();
  if (options?.task) params.set("task", options.task);
  if (options?.limit !== undefined) params.set("limit", String(options.limit));
  const query = params.toString();
  return apiGet<{ runs: TaskRun[] }>(`/api/task-runs${query ? `?${query}` : ""}`);
}

export function fetchRunLogs(runId: string): Promise<RunLogs> {
  return apiGet<RunLogs>(`/api/task-runs/${encodeURIComponent(runId)}/logs`);
}

export function runLogStreamUrl(runId: string): string {
  return `/api/task-runs/${encodeURIComponent(runId)}/logs/stream`;
}

export function runTaskRequest(
  name: string,
  options?: ManualRunOptions,
): Promise<{ runId: string }> {
  if (options?.maxRecommendations !== undefined) {
    return apiPost<{ runId: string }>("/api/recommendations/run", {
      maxRecommendations: options.maxRecommendations,
    });
  }
  return apiPost<{ runId: string }>(`/api/tasks/${encodeURIComponent(name)}/run`);
}

export function fetchStreamerMetrics(id: string): Promise<StreamerMetrics> {
  return apiGet<StreamerMetrics>(`/api/streamers/${encodeURIComponent(id)}/metrics`);
}

export function fetchRecommendations(): Promise<{
  recommendations: Recommendation[];
}> {
  return apiGet<{ recommendations: Recommendation[] }>("/api/recommendations");
}

export function fetchTasteProfile(): Promise<{ profile: TasteProfile | null }> {
  return apiGet<{ profile: TasteProfile | null }>(
    "/api/recommendations/taste-profile",
  );
}

export function sendRecommendationFeedback(
  recommendationId: string,
  feedback: RecommendationFeedback,
): Promise<{ recommendation: Recommendation }> {
  return apiPost<{ recommendation: Recommendation }>(
    `/api/recommendations/${encodeURIComponent(recommendationId)}/feedback`,
    { feedback },
  );
}

export function fetchPodcastRecommendations(): Promise<{
  recommendations: PodcastRecommendation[];
}> {
  return apiGet<{ recommendations: PodcastRecommendation[] }>(
    "/api/podcast-recommendations",
  );
}

export function sendPodcastRecommendationFeedback(
  recommendationId: string,
  feedback: PodcastFeedback,
): Promise<{ recommendation: PodcastRecommendation }> {
  return apiPost<{ recommendation: PodcastRecommendation }>(
    `/api/podcast-recommendations/${encodeURIComponent(recommendationId)}/feedback`,
    { feedback },
  );
}
