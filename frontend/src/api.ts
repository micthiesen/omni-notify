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
  /** UI label; falls back to toTitleCase(name) when absent. */
  displayName?: string | null;
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

export interface StreamSession {
  startedAt: number;
  endedAt: number;
  durationMs: number;
  peakViewers: number;
  title: string;
  platform: string;
  username: string;
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
  feedbackNote: string | null;
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

export interface PodcastTasteStats {
  listenedEpisodes: number;
  startedEpisodes: number;
  starredEpisodes: number;
  distinctShows: number;
  recommendations: {
    total: number;
    listened: number;
    abandoned: number;
    ignored: number;
    failed: number;
    awaitingOutcome: number;
  };
  feedback: {
    goodPick: number;
    notForMe: number;
  };
}

export interface PodcastTasteProfile {
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
  stats: PodcastTasteStats;
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
  feedbackNote?: string | null;
}

export type EmailPipeline = "ParcelTracker" | "CalendarEvents";
export type EmailActivityOutcome =
  | "filtered"
  | "skipped"
  | "no_matches"
  | "processed"
  | "partial"
  | "failed"
  | "error";

export interface EmailActivity {
  activityId: string;
  pipeline: EmailPipeline;
  emailId: string;
  subject: string;
  from: string;
  receivedAt: number;
  processedAt: number;
  outcome: EmailActivityOutcome;
  /** Why the filter admitted this email, e.g. "triage: mentions UPS tracking". */
  admitReason: string | null;
  /** Which tier admitted it: rule/builtin/triage/keyword-fallback/carrier-name. */
  admitTier: string | null;
  /** LLM cost (cents) attributable to this row; null when no priced LLM ran. */
  costCents: number | null;
  detail: string | null;
  items: string[];
}

export type EmailRuleScope = "parcel" | "calendar" | "both";
export type EmailRuleVerdict = "block" | "allow";

export interface EmailRule {
  ruleId: string;
  /** Lowercase full address ("x@y.com") or bare domain ("y.com"). */
  pattern: string;
  scope: EmailRuleScope;
  verdict: EmailRuleVerdict;
  createdAt: number;
}

export type EmailFeedbackVerdict = "not_relevant" | "missed";

export interface EmailFeedback {
  activityId: string;
  pipeline: EmailPipeline;
  emailId: string;
  subject: string;
  from: string;
  verdict: EmailFeedbackVerdict;
  note?: string;
  createdAt: number;
}

export interface BriefingNotification {
  title: string;
  message: string;
  url: string;
  timestamp: number;
  /** Task run that produced this notification, for opening its logs. */
  runId: string | null;
  /** LLM cost (cents) of producing it; null when unpriced/uncomputed. */
  costCents: number | null;
}

export interface BriefingHistory {
  name: string;
  notifications: BriefingNotification[];
}

export type PressPodsRetrieverAttempt =
  | { name: string; success: true; contentRating: number; textChars: number }
  | { name: string; success: false; error: string };

export interface PressPodsChapter {
  startTimeSeconds: number;
  title: string;
}

export interface PressPodsChunkStat {
  index: number;
  sectionIndex: number;
  sectionTitle?: string;
  text: string;
  charCount: number;
  durationSeconds: number;
  startTimeSeconds: number;
  secPerChar: number;
  attempts: number;
  /** STT content-verification of the chosen take (Higgs only, when an STT
   * endpoint is configured). `coverage` is the fraction of input words recovered
   * from the audio (~1 complete, low = truncated). Absent on older episodes. */
  coverage?: number;
  wordRatio?: number;
  /** This piece came from re-splitting a larger chunk that kept failing
   * verification. Recovery worked, but marks where Higgs struggled. */
  resplit?: boolean;
}

export interface PressPodsCosts {
  llmCents: number;
  ttsCents: number;
  detailCents: Record<string, number>;
  detailTokens: Record<string, { input: number; output: number }>;
  detailChars: Record<string, number>;
}

export interface PressPodsEpisode {
  episodeId: string;
  title: string;
  author: string | null;
  publication: string | null;
  domain: string | null;
  articleUrl: string;
  leadImageUrl: string | null;
  excerpt: string | null;
  voiceName: string | null;
  synthesizedSeconds: number | null;
  audioUrl: string;
  durationSeconds: number | null;
  fileBytes: number;
  retrieverName: string | null;
  retrieverSeconds: number | null;
  retrieverAttempts: PressPodsRetrieverAttempt[] | null;
  chapters: PressPodsChapter[] | null;
  costCents: number | null;
  createdAt: number;
  publishedAt: number | null;
  runId: string | null;
}

/** Full per-episode detail from GET /api/press-pods/episodes/:id. */
export interface PressPodsEpisodeDetail extends PressPodsEpisode {
  content: string;
  authorGender: string | null;
  voiceProvider: string | null;
  chunks: PressPodsChunkStat[] | null;
  costs: PressPodsCosts | null;
}

export type PressPodsJobStatus = "queued" | "processing" | "failed";

export interface PressPodsJob {
  jobId: string;
  url: string;
  status: PressPodsJobStatus;
  attempts: number;
  nextAttemptAt: number | null;
  lastError: string | null;
  createdAt: number;
  updatedAt: number;
  lastRunId: string | null;
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

// Container restarts are routine and take up to ~30s; during one, fetches
// either fail at the network layer or hit the proxy's 502/503/504. GETs are
// idempotent, so ride out restarts with backoff instead of erroring pages
// into blank states. Application errors (4xx, 500) still surface immediately.
const RETRYABLE_STATUS = new Set([502, 503, 504]);
const GET_RETRY_DELAYS_MS = [500, 1_000, 2_000, 4_000, 8_000, 8_000, 8_000];

async function fetchGetWithRetry(path: string): Promise<Response> {
  for (let attempt = 0; ; attempt++) {
    const lastAttempt = attempt >= GET_RETRY_DELAYS_MS.length;
    try {
      const res = await fetch(path);
      if (!RETRYABLE_STATUS.has(res.status) || lastAttempt) return res;
    } catch (err) {
      // Network failure: server down mid-restart
      if (lastAttempt) throw err;
    }
    await new Promise((resolve) => setTimeout(resolve, GET_RETRY_DELAYS_MS[attempt]));
  }
}

export async function apiGet<T>(path: string): Promise<T> {
  const res = await fetchGetWithRetry(path);
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

export async function apiDelete<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method: "DELETE",
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

export function fetchStreamerSessions(
  id: string,
): Promise<{ sessions: StreamSession[] }> {
  return apiGet<{ sessions: StreamSession[] }>(
    `/api/streamers/${encodeURIComponent(id)}/sessions`,
  );
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

export function fetchPressPods(): Promise<{
  episodes: PressPodsEpisode[];
  jobs: PressPodsJob[];
}> {
  return apiGet("/api/press-pods/episodes");
}

export function fetchPressPodsEpisode(
  episodeId: string,
): Promise<{ episode: PressPodsEpisodeDetail }> {
  return apiGet<{ episode: PressPodsEpisodeDetail }>(
    `/api/press-pods/episodes/${encodeURIComponent(episodeId)}`,
  );
}

export function submitPressPodsUrl(url: string): Promise<{ job: PressPodsJob }> {
  return apiPost("/api/press-pods/submit", { url });
}

export function retryPressPodsJob(jobId: string): Promise<{ job: PressPodsJob }> {
  return apiPost(`/api/press-pods/jobs/${encodeURIComponent(jobId)}/retry`);
}

export function dismissPressPodsJob(jobId: string): Promise<{ deleted: boolean }> {
  return apiDelete(`/api/press-pods/jobs/${encodeURIComponent(jobId)}`);
}

export function fetchBriefings(): Promise<{ briefings: BriefingHistory[] }> {
  return apiGet<{ briefings: BriefingHistory[] }>("/api/briefings");
}

export function fetchEmailActivity(
  pipeline?: EmailPipeline,
  limit?: number,
): Promise<{ activities: EmailActivity[] }> {
  const params = new URLSearchParams();
  if (pipeline) params.set("pipeline", pipeline);
  if (limit !== undefined) params.set("limit", String(limit));
  const query = params.toString();
  return apiGet<{ activities: EmailActivity[] }>(
    `/api/email-activity${query ? `?${query}` : ""}`,
  );
}

export interface EmailActivityLogs {
  activity: EmailActivity;
  lines: RunLogLine[];
  dropped: number;
}

export function fetchEmailActivityLogs(
  activityId: string,
): Promise<EmailActivityLogs> {
  return apiGet<EmailActivityLogs>(
    `/api/email-activity/${encodeURIComponent(activityId)}/logs`,
  );
}

export interface EmailBuiltinRules {
  parcel: { blocked: string[]; autoPass: string[] };
  calendar: { blocked: string[]; autoPass: string[] };
}

export function fetchEmailRules(): Promise<{
  rules: EmailRule[];
  builtin: EmailBuiltinRules;
}> {
  return apiGet<{ rules: EmailRule[]; builtin: EmailBuiltinRules }>(
    "/api/email-rules",
  );
}

export type CreateEmailRuleStatus = "created" | "exists" | "merged" | "builtin";

export interface CreateEmailRuleResult {
  rule?: EmailRule;
  status: CreateEmailRuleStatus;
  message?: string;
}

export function createEmailRule(input: {
  pattern: string;
  scope: EmailRuleScope;
  verdict: EmailRuleVerdict;
}): Promise<CreateEmailRuleResult> {
  return apiPost<CreateEmailRuleResult>("/api/email-rules", input);
}

export function deleteEmailRule(ruleId: string): Promise<{ deleted: true }> {
  return apiDelete<{ deleted: true }>(
    `/api/email-rules/${encodeURIComponent(ruleId)}`,
  );
}

export function fetchEmailFeedback(): Promise<{ feedback: EmailFeedback[] }> {
  return apiGet<{ feedback: EmailFeedback[] }>("/api/email-feedback");
}

export function sendEmailActivityFeedback(
  activityId: string,
  verdict: EmailFeedbackVerdict | null,
  note?: string,
): Promise<{ feedback: EmailFeedback | null }> {
  return apiPost<{ feedback: EmailFeedback | null }>(
    `/api/email-activity/${encodeURIComponent(activityId)}/feedback`,
    { verdict, ...(note === undefined ? {} : { note }) },
  );
}

export function reprocessEmailActivity(
  activityId: string,
): Promise<{ activity: EmailActivity }> {
  return apiPost<{ activity: EmailActivity }>(
    `/api/email-activity/${encodeURIComponent(activityId)}/reprocess`,
  );
}

export function forgetParcelDelivery(
  trackingNumber: string,
): Promise<{ deleted: true }> {
  return apiDelete<{ deleted: true }>(
    `/api/parcel-tracker/deliveries/${encodeURIComponent(trackingNumber)}`,
  );
}

export function fetchRecommendation(
  recommendationId: string,
): Promise<{ recommendation: Recommendation }> {
  return apiGet<{ recommendation: Recommendation }>(
    `/api/recommendations/${encodeURIComponent(recommendationId)}`,
  );
}

export function fetchPodcastRecommendation(
  recommendationId: string,
): Promise<{ recommendation: PodcastRecommendation }> {
  return apiGet<{ recommendation: PodcastRecommendation }>(
    `/api/podcast-recommendations/${encodeURIComponent(recommendationId)}`,
  );
}

export function sendRecommendationFeedback(
  recommendationId: string,
  feedback: RecommendationFeedback | null,
  note?: string,
): Promise<{ recommendation: Recommendation }> {
  return apiPost<{ recommendation: Recommendation }>(
    `/api/recommendations/${encodeURIComponent(recommendationId)}/feedback`,
    {
      ...(feedback ? { feedback } : {}),
      ...(note === undefined ? {} : { note }),
    },
  );
}

export function fetchPodcastRecommendations(): Promise<{
  recommendations: PodcastRecommendation[];
}> {
  return apiGet<{ recommendations: PodcastRecommendation[] }>(
    "/api/podcast-recommendations",
  );
}

export function fetchPodcastTasteProfile(): Promise<{
  profile: PodcastTasteProfile | null;
}> {
  return apiGet<{ profile: PodcastTasteProfile | null }>(
    "/api/podcast-recommendations/taste-profile",
  );
}

export function sendPodcastRecommendationFeedback(
  recommendationId: string,
  feedback: PodcastFeedback | null,
  note?: string,
): Promise<{ recommendation: PodcastRecommendation }> {
  return apiPost<{ recommendation: PodcastRecommendation }>(
    `/api/podcast-recommendations/${encodeURIComponent(recommendationId)}/feedback`,
    {
      ...(feedback ? { feedback } : {}),
      ...(note === undefined ? {} : { note }),
    },
  );
}
