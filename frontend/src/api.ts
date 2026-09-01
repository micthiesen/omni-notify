import { Data, Effect, Schedule, Schema } from "effect";

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
  /** Presence in Destiny.gg's current embeds, when DGG discovery is enabled. */
  dgg?: {
    hosted: boolean;
    viewers: number | null;
  };
}

export type LivestreamAlertType =
  | "destiny_guest"
  | "breaking_news"
  | "debate"
  | "guest_joined"
  | "major_announcement"
  | "viewer_surge"
  | "cross_stream_topic";

export interface LivestreamIntelligence {
  streamerId: string;
  sessionStartedAt: number;
  semantic?: {
    headline: string;
    topics: string[];
    contentKind: string;
    importance: number;
    reason: string;
    updatedAt: number;
  };
  trend?: {
    percentChange: number;
    viewersPerMinute: number;
    dggPercentChange: number | null;
    anomalous: boolean;
    reason: string | null;
    currentViewers?: number | null;
    baselineViewers?: number | null;
    currentDggViewers?: number | null;
    baselineDggViewers?: number | null;
    baselineSamples?: number;
    candidateObservations?: number;
    suppressionReason?: string | null;
    updatedAt: number;
  };
  relevanceScore: number;
  relevanceReasons: string[];
  summary?: {
    text: string;
    topic: string;
    confidence: number;
    transcriptExcerpt: string;
    updatedAt: number;
    windowSeconds: number;
  };
  chapters: Array<{
    chapterId: string;
    startedAt: number;
    title: string;
    summary: string;
  }>;
  destinyPresence?: {
    state: "possible" | "confirmed";
    confidence: number;
    detectedAt: number;
    reason: string;
  };
  latestAlert?: {
    alertId: string;
    type: LivestreamAlertType;
    title: string;
    message: string;
    reason: string;
    confidence: number;
    createdAt: number;
  };
  updatedAt: number;
}

export type LivestreamPipelineStage = "metadata" | "voice" | "summary" | "alert";
export type LivestreamPipelineStatus =
  | "idle"
  | "running"
  | "success"
  | "skipped"
  | "error";

export interface LivestreamStageDiagnostic {
  status: LivestreamPipelineStatus;
  eligible?: boolean;
  startedAt?: number;
  finishedAt?: number;
  nextAt?: number;
  durationMs?: number;
  detail?: string;
  metrics?: Record<string, number | string | boolean | null>;
}

export interface LivestreamDiagnostics {
  streamerId: string;
  sessionStartedAt?: number;
  stages: Partial<Record<LivestreamPipelineStage, LivestreamStageDiagnostic>>;
  updatedAt: number;
}

export type LivestreamEventKind =
  | "session"
  | "metadata"
  | "voice"
  | "summary"
  | "alert"
  | "feedback"
  | "anomaly";

export interface LivestreamIntelligenceEvent {
  eventId: string;
  streamerId: string;
  sessionStartedAt?: number;
  createdAt: number;
  kind: LivestreamEventKind;
  status: "info" | "success" | "warning" | "error";
  title: string;
  detail?: string;
  durationMs?: number;
  costCents?: number;
  metrics?: Record<string, number | string | boolean | null>;
}

export interface LivestreamRuntimeDiagnostics {
  enabled: true;
  voiceprintLoaded: boolean;
  model: string;
  queues: Record<"capture" | "speech" | "llm", { running: number; queued: number }>;
  activeStreamCount: number;
  activeVoiceTargetCount: number;
  budget: { spentCents: number; limitCents: number; remainingCents: number };
  intervals: { voiceSeconds: number; summarySeconds: number };
}

export interface LivestreamIntelligenceDetails {
  intelligence: LivestreamIntelligence | null;
  diagnostics: LivestreamDiagnostics | null;
  events: LivestreamIntelligenceEvent[];
  runtime: LivestreamRuntimeDiagnostics | null;
  generatedAt: number;
}

export interface LivestreamFeedback {
  feedbackId: string;
  streamerId: string;
  alertId: string;
  alertType: LivestreamAlertType;
  verdict: "useful" | "not_useful" | "false_positive";
  note?: string;
  createdAt: number;
}

export type StreamerTier = "primary" | "background";

export type LiveStreamer = StreamerBase & {
  live: true;
  title: string;
  startedAt: number;
  maxViewerCount: number;
  /** Current viewer count, when the platform reports it live. */
  viewerCount: number | null;
  /** Per-platform observations contributing to viewerCount. */
  sources: Array<{
    platform: string;
    username: string;
    title: string;
    viewerCount: number | null;
    category?: string;
  }>;
  /** Twitch/Kick category or game name, when reported. */
  category: string | null;
  tier: StreamerTier;
  primary: StreamerBinding;
  intelligence?: LivestreamIntelligence;
};

export type OfflineStreamer = StreamerBase & {
  live: false;
  lastStartedAt: number | null;
  lastEndedAt: number | null;
  lastMaxViewerCount: number | null;
  tier: StreamerTier;
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
  platforms: Array<{
    platform: string;
    username: string;
    dailyBuckets: DailyViewerBucket[];
    allTimeMax: number;
    allTimeMaxTimestamp: number;
  }>;
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

/** Newest delivered-but-unwatched media recommendation, for the home page
 * "what else could I watch" strip. Already filtered/sorted/capped server-side. */
export interface OnDeckItem {
  recommendationId: string;
  title: string;
  mediaType: MediaType;
  year: number | null;
  posterPath: string | null;
  whyForUser: string | null;
  recommendedAt: number;
}

export interface Snapshot {
  tasks: TaskInfo[];
  streamers: StreamerView[];
  runs: TaskRun[];
  onDeck: OnDeckItem[];
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
export type RecommendationFeedback = "good_pick" | "not_for_me" | "already_watched";

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
  itunesId: number | null;
  artworkUrl: string | null;
  episodeUrl: string | null;
  publishedAt: number;
  durationMinutes: number | null;
  status: PodcastRecommendationStatus;
  whyForUser: string | null;
  caveats: string[];
  confidence: number | null;
  shortlistScores: {
    tasteMatch: number;
    novelty: number;
    composite: number;
    risks: string[];
  } | null;
  discoveredVia: string | null;
  sourceUrl: string | null;
  matchedVoices: string[];
  recommendedAt: number;
  notifiedAt: number | null;
  queueResult: PodcastQueueResult | null;
  feedback: PodcastFeedback | null;
  feedbackAt: number | null;
  feedbackNote: string | null;
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
  expectedWords?: number;
  /** This piece came from re-splitting a larger chunk that kept failing
   * verification. Recovery worked, but marks where Higgs struggled. */
  resplit?: boolean;
  /** Number of adaptive re-split levels used to produce this piece. */
  resplitDepth?: number;
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

export type CostRange = 7 | 30 | 90 | "all";

export interface CostSummary {
  selectedCostCents: number;
  allTimeCostCents: number;
  allTimeUnknownEventCount: number;
  averageDailyCostCents: number;
  highestDay: { date: string; costCents: number } | null;
  eventCount: number;
  unknownEventCount: number;
  inputTokens: number;
  outputTokens: number;
  characters: number;
  requests: number;
  credits: number;
}

export interface DailyCost {
  /** YYYY-MM-DD */
  date: string;
  costCents: number;
  byFeature: Record<string, number>;
  pricedEventCount: number;
  unknownEventCount: number;
}

export interface FeatureCost {
  feature: string;
  costCents: number;
  eventCount: number;
  unknownEventCount: number;
}

export interface ServiceCost {
  service: string;
  model: string | null;
  category: string;
  costCents: number;
  eventCount: number;
  unknownEventCount: number;
  inputTokens: number;
  outputTokens: number;
  characters: number;
  requests: number;
  credits: number;
}

export interface CostEvent {
  eventId: string;
  incurredAt: number;
  feature: string;
  operation: string;
  service: string;
  model: string | null;
  category: string;
  costCents: number | null;
  priceStatus: string;
  runId: string | null;
}

export interface CostsResponse {
  range: { days: number | null; from: number | null; to: number };
  summary: CostSummary;
  daily: DailyCost[];
  byFeature: FeatureCost[];
  byService: ServiceCost[];
  recent: CostEvent[];
}

export type WorkspaceSubjectStatus = "active" | "paused" | "completed" | "archived";
export interface WorkspaceArtifactDefinition {
  key: string;
  title: string;
  kind: string;
  instructions: string;
}
export interface WorkspaceDefinition {
  id: string;
  title: string;
  description: string;
  subjectLabel: string;
  subjectLabelPlural: string;
  taskName: string;
  schedule: string;
  scheduledRuns?: boolean;
  inputPlaceholder?: string;
  followUpPlaceholder?: string;
  instructions: string;
  artifacts: WorkspaceArtifactDefinition[];
}
export interface WorkspaceSubject {
  workspaceId: string;
  subjectId: string;
  title: string;
  status: WorkspaceSubjectStatus;
  summary: string;
  createdAt: number;
  updatedAt: number;
  lastResearchedAt?: number;
}
export interface WorkspaceSummary extends WorkspaceDefinition {
  subjects: WorkspaceSubject[];
  activeSubjectCount: number;
  pendingActionCount: number;
  openPapercutCount: number;
}
export interface WorkspaceArtifact {
  revisionId: string;
  workspaceId: string;
  subjectId: string;
  artifactKey: string;
  kind: string;
  content: string;
  summary: string;
  createdAt: number;
  runId?: string;
}
export interface WorkspaceMessage {
  messageId: string;
  role: "user" | "assistant" | "system";
  text: string;
  createdAt: number;
  runId?: string;
}
export interface WorkspaceSource {
  sourceId: string;
  kind: "web" | "email";
  title: string;
  url?: string;
  excerpt: string;
  createdAt: number;
}
export interface WorkspaceAction {
  actionId: string;
  type: "email_scope" | "calendar_event";
  status: "pending" | "approved" | "rejected" | "failed";
  title: string;
  description: string;
  payload: string;
  createdAt: number;
  result?: string;
}
export interface WorkspacePapercut {
  papercutId: string;
  category: string;
  title: string;
  detail: string;
  occurrences: number;
  lastSeenAt: number;
  status: "open" | "addressed" | "dismissed";
}
export interface WorkspaceDetailResponse {
  workspace: WorkspaceDefinition;
  subject: WorkspaceSubject;
  artifacts: WorkspaceArtifact[];
  artifactRevisions: WorkspaceArtifact[];
  messages: WorkspaceMessage[];
  sources: WorkspaceSource[];
  actions: WorkspaceAction[];
  emailScope: {
    senders: string[];
    domains: string[];
    subjectKeywords: string[];
    bodyKeywords: string[];
  } | null;
  papercuts: WorkspacePapercut[];
}

export class ApiError extends Data.TaggedError("ApiError")<{
  readonly status: number;
  readonly message: string;
}> {}

export class ApiNetworkError extends Data.TaggedError("ApiNetworkError")<{
  readonly path: string;
  readonly message: string;
  readonly cause: unknown;
}> {}

export class ApiDecodeError extends Data.TaggedError("ApiDecodeError")<{
  readonly path: string;
  readonly message: string;
  readonly cause: unknown;
}> {}

export type ApiClientError = ApiError | ApiNetworkError | ApiDecodeError;

class RetryableGetError extends Data.TaggedError("RetryableGetError")<{
  readonly response: Response;
}> {}

const ErrorBodySchema = Schema.Struct({ error: Schema.String });

const extractErrorMessage = (res: Response): Effect.Effect<string> =>
  Effect.tryPromise({
    try: () => res.clone().json() as Promise<unknown>,
    catch: () => undefined,
  }).pipe(
    Effect.flatMap((body) => Schema.decodeUnknownEffect(ErrorBodySchema)(body)),
    Effect.map((body) => body.error),
    Effect.catch(() => Effect.succeed(`HTTP ${res.status}: ${res.statusText}`)),
  );

// Container restarts are routine and take up to ~30s; during one, fetches
// either fail at the network layer or hit the proxy's 502/503/504. GETs are
// idempotent, so ride out restarts with backoff instead of erroring pages
// into blank states. Application errors (4xx, 500) still surface immediately.
const RETRYABLE_STATUS = new Set([502, 503, 504]);
const getRetrySchedule = Schedule.max([
  Schedule.min([Schedule.exponential("500 millis"), Schedule.spaced("8 seconds")]),
  Schedule.recurs(7),
]);

const fetchResponse = (
  path: string,
  init?: RequestInit,
): Effect.Effect<Response, ApiNetworkError> =>
  Effect.tryPromise({
    try: (signal) => fetch(path, { ...init, signal }),
    catch: (cause) =>
      new ApiNetworkError({ path, message: `Request failed: ${path}`, cause }),
  });

const fetchGetWithRetry = (path: string): Effect.Effect<Response, ApiNetworkError> =>
  fetchResponse(path).pipe(
    Effect.flatMap((response) =>
      RETRYABLE_STATUS.has(response.status)
        ? Effect.fail(new RetryableGetError({ response }))
        : Effect.succeed(response),
    ),
    Effect.retry({ schedule: getRetrySchedule }),
    Effect.catchTag("RetryableGetError", ({ response }) => Effect.succeed(response)),
  );

const decodeResponse = <A>(
  path: string,
  response: Response,
  schema: Schema.Decoder<A, never>,
): Effect.Effect<A, ApiError | ApiDecodeError> =>
  Effect.gen(function* () {
    if (!response.ok) {
      return yield* new ApiError({
        status: response.status,
        message: yield* extractErrorMessage(response),
      });
    }
    const body = yield* Effect.tryPromise({
      try: () => response.json() as Promise<unknown>,
      catch: (cause) =>
        new ApiDecodeError({ path, message: `Invalid JSON response: ${path}`, cause }),
    });
    return yield* Schema.decodeUnknownEffect(schema)(body).pipe(
      Effect.mapError(
        (cause) =>
          new ApiDecodeError({
            path,
            message: `Response did not match its schema: ${path}`,
            cause,
          }),
      ),
    );
  });

export const apiGet = <A>(
  path: string,
  schema: Schema.Decoder<A, never>,
): Effect.Effect<A, ApiClientError> =>
  fetchGetWithRetry(path).pipe(
    Effect.flatMap((response) => decodeResponse(path, response, schema)),
  );

const apiWrite = <A>(
  method: "POST" | "DELETE",
  path: string,
  schema: Schema.Decoder<A, never>,
  body?: unknown,
): Effect.Effect<A, ApiClientError> =>
  fetchResponse(path, {
    method,
    ...(body === undefined
      ? {}
      : {
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        }),
  }).pipe(Effect.flatMap((response) => decodeResponse(path, response, schema)));

export const apiPost = <A>(
  path: string,
  schema: Schema.Decoder<A, never>,
  body?: unknown,
): Effect.Effect<A, ApiClientError> => apiWrite("POST", path, schema, body);

export const apiDelete = <A>(
  path: string,
  schema: Schema.Decoder<A, never>,
  body?: unknown,
): Effect.Effect<A, ApiClientError> => apiWrite("DELETE", path, schema, body);

const RunIdResponse = Schema.Struct({ runId: Schema.String });
const DeletedBooleanResponse = Schema.Struct({ deleted: Schema.Boolean });
const DeletedTrueResponse = Schema.Struct({ deleted: Schema.Literal(true) });

// Effect Schema models decoded collections as readonly. The browser state in this
// existing React app uses mutable interfaces, while JSON decoding still returns
// ordinary mutable arrays. This bridge changes only the static collection view;
// every supplied schema below remains concrete and performs full runtime decoding.
const browserSchema = <A>(schema: Schema.Top): Schema.Decoder<A, never> =>
  schema as Schema.Decoder<A, never>;
const listResponseSchema = <K extends string, A>(
  key: K,
  item: Schema.Schema<A>,
): Schema.Decoder<Record<K, A[]>, never> =>
  browserSchema(Schema.Struct({ [key]: Schema.Array(item) }));
const itemResponseSchema = <K extends string, A>(
  key: K,
  item: Schema.Schema<A>,
): Schema.Decoder<Record<K, A>, never> => browserSchema(Schema.Struct({ [key]: item }));

export const TaskRunSchema = browserSchema<TaskRun>(
  Schema.Struct({
    runId: Schema.String,
    taskName: Schema.String,
    trigger: Schema.Literals(["schedule", "manual", "startup", "catchup"]),
    scheduledFor: Schema.NullOr(Schema.Number),
    startedAt: Schema.Number,
    finishedAt: Schema.NullOr(Schema.Number),
    status: Schema.Literals(["running", "success", "error"]),
    error: Schema.NullOr(Schema.String),
    summary: Schema.NullOr(Schema.String),
  }),
);

export const TaskInfoSchema = browserSchema<TaskInfo>(
  Schema.Struct({
    name: Schema.String,
    displayName: Schema.optional(Schema.NullOr(Schema.String)),
    schedule: Schema.String,
    running: Schema.Boolean,
    nextRuns: Schema.Array(Schema.String),
    lastRun: Schema.NullOr(TaskRunSchema),
  }),
);

const StreamerBindingSchema = Schema.Struct({
  platform: Schema.String,
  username: Schema.String,
  url: Schema.String,
});
const DggPresenceSchema = Schema.Struct({
  hosted: Schema.Boolean,
  viewers: Schema.NullOr(Schema.Number),
});
const LivestreamSemanticSchema = Schema.Struct({
  headline: Schema.String,
  topics: Schema.Array(Schema.String),
  contentKind: Schema.String,
  importance: Schema.Number,
  reason: Schema.String,
  updatedAt: Schema.Number,
});
const LivestreamTrendSchema = Schema.Struct({
  percentChange: Schema.Number,
  viewersPerMinute: Schema.Number,
  dggPercentChange: Schema.NullOr(Schema.Number),
  anomalous: Schema.Boolean,
  reason: Schema.NullOr(Schema.String),
  currentViewers: Schema.optional(Schema.NullOr(Schema.Number)),
  baselineViewers: Schema.optional(Schema.NullOr(Schema.Number)),
  currentDggViewers: Schema.optional(Schema.NullOr(Schema.Number)),
  baselineDggViewers: Schema.optional(Schema.NullOr(Schema.Number)),
  baselineSamples: Schema.optional(Schema.Number),
  candidateObservations: Schema.optional(Schema.Number),
  suppressionReason: Schema.optional(Schema.NullOr(Schema.String)),
  updatedAt: Schema.Number,
});
const LivestreamSummarySchema = Schema.Struct({
  text: Schema.String,
  topic: Schema.String,
  confidence: Schema.Number,
  transcriptExcerpt: Schema.String,
  updatedAt: Schema.Number,
  windowSeconds: Schema.Number,
});
const LivestreamChapterSchema = Schema.Struct({
  chapterId: Schema.String,
  startedAt: Schema.Number,
  title: Schema.String,
  summary: Schema.String,
});
const LivestreamAlertSchema = Schema.Struct({
  alertId: Schema.String,
  type: Schema.Literals([
    "destiny_guest",
    "breaking_news",
    "debate",
    "guest_joined",
    "major_announcement",
    "viewer_surge",
    "cross_stream_topic",
  ]),
  title: Schema.String,
  message: Schema.String,
  reason: Schema.String,
  confidence: Schema.Number,
  createdAt: Schema.Number,
});
const LivestreamFeedbackSchema = Schema.Struct({
  feedbackId: Schema.String,
  streamerId: Schema.String,
  alertId: Schema.String,
  alertType: Schema.Literals([
    "destiny_guest",
    "breaking_news",
    "debate",
    "guest_joined",
    "major_announcement",
    "viewer_surge",
    "cross_stream_topic",
  ]),
  verdict: Schema.Literals(["useful", "not_useful", "false_positive"]),
  note: Schema.optional(Schema.String),
  createdAt: Schema.Number,
});
export const LivestreamIntelligenceSchema = browserSchema<LivestreamIntelligence>(
  Schema.Struct({
    streamerId: Schema.String,
    sessionStartedAt: Schema.Number,
    semantic: Schema.optional(LivestreamSemanticSchema),
    trend: Schema.optional(LivestreamTrendSchema),
    relevanceScore: Schema.Number,
    relevanceReasons: Schema.Array(Schema.String),
    summary: Schema.optional(LivestreamSummarySchema),
    chapters: Schema.Array(LivestreamChapterSchema),
    destinyPresence: Schema.optional(
      Schema.Struct({
        state: Schema.Literals(["possible", "confirmed"]),
        confidence: Schema.Number,
        detectedAt: Schema.Number,
        reason: Schema.String,
      }),
    ),
    latestAlert: Schema.optional(LivestreamAlertSchema),
    updatedAt: Schema.Number,
  }),
);
const StreamerBaseFields = {
  id: Schema.String,
  displayName: Schema.String,
  bindings: Schema.Array(StreamerBindingSchema),
  dgg: Schema.optional(DggPresenceSchema),
} as const;
export const LiveStreamerSchema = browserSchema<LiveStreamer>(
  Schema.Struct({
    ...StreamerBaseFields,
    live: Schema.Literal(true),
    title: Schema.String,
    startedAt: Schema.Number,
    maxViewerCount: Schema.Number,
    viewerCount: Schema.NullOr(Schema.Number),
    sources: Schema.Array(
      Schema.Struct({
        platform: Schema.String,
        username: Schema.String,
        title: Schema.String,
        viewerCount: Schema.NullOr(Schema.Number),
        category: Schema.optional(Schema.String),
      }),
    ),
    category: Schema.NullOr(Schema.String),
    tier: Schema.Literals(["primary", "background"]),
    primary: StreamerBindingSchema,
    intelligence: Schema.optional(LivestreamIntelligenceSchema),
  }),
);
export const OfflineStreamerSchema = browserSchema<OfflineStreamer>(
  Schema.Struct({
    ...StreamerBaseFields,
    live: Schema.Literal(false),
    lastStartedAt: Schema.NullOr(Schema.Number),
    lastEndedAt: Schema.NullOr(Schema.Number),
    lastMaxViewerCount: Schema.NullOr(Schema.Number),
    tier: Schema.Literals(["primary", "background"]),
  }),
);
export const StreamerViewSchema = browserSchema<StreamerView>(
  Schema.Union([LiveStreamerSchema, OfflineStreamerSchema]),
);

const OnDeckItemSchema = Schema.Struct({
  recommendationId: Schema.String,
  title: Schema.String,
  mediaType: Schema.Literals(["movie", "tv"]),
  year: Schema.NullOr(Schema.Number),
  posterPath: Schema.NullOr(Schema.String),
  whyForUser: Schema.NullOr(Schema.String),
  recommendedAt: Schema.Number,
});
export const SnapshotSchema = browserSchema<Snapshot>(
  Schema.Struct({
    tasks: Schema.Array(TaskInfoSchema),
    streamers: Schema.Array(StreamerViewSchema),
    runs: Schema.Array(TaskRunSchema),
    onDeck: Schema.Array(OnDeckItemSchema),
  }),
);
export const RunLogLineSchema = browserSchema<RunLogLine>(
  Schema.Struct({
    t: Schema.Number,
    level: Schema.Literals(["debug", "info", "warn", "error"]),
    logger: Schema.String,
    msg: Schema.String,
  }),
);
export const RunLogsSchema = browserSchema<RunLogs>(
  Schema.Struct({
    run: TaskRunSchema,
    lines: Schema.Array(RunLogLineSchema),
    dropped: Schema.Number,
  }),
);

const DailyViewerBucketSchema = Schema.Struct({
  date: Schema.String,
  maxViewers: Schema.Number,
  timestamp: Schema.Number,
});
export const StreamerMetricsSchema = browserSchema<StreamerMetrics>(
  Schema.Struct({
    dailyBuckets: Schema.Array(DailyViewerBucketSchema),
    allTimeMax: Schema.Number,
    allTimeMaxTimestamp: Schema.Number,
    platforms: Schema.Array(
      Schema.Struct({
        platform: Schema.String,
        username: Schema.String,
        dailyBuckets: Schema.Array(DailyViewerBucketSchema),
        allTimeMax: Schema.Number,
        allTimeMaxTimestamp: Schema.Number,
      }),
    ),
  }),
);
export const StreamSessionSchema = browserSchema<StreamSession>(
  Schema.Struct({
    startedAt: Schema.Number,
    endedAt: Schema.Number,
    durationMs: Schema.Number,
    peakViewers: Schema.Number,
    title: Schema.String,
    platform: Schema.String,
    username: Schema.String,
  }),
);

const DiagnosticMetricValueSchema = Schema.Union([
  Schema.Number,
  Schema.String,
  Schema.Boolean,
  Schema.Null,
]);
const DiagnosticMetricsSchema = Schema.Record(
  Schema.String,
  DiagnosticMetricValueSchema,
);
const StageDiagnosticSchema = Schema.Struct({
  status: Schema.Literals(["idle", "running", "success", "skipped", "error"]),
  eligible: Schema.optional(Schema.Boolean),
  startedAt: Schema.optional(Schema.Number),
  finishedAt: Schema.optional(Schema.Number),
  nextAt: Schema.optional(Schema.Number),
  durationMs: Schema.optional(Schema.Number),
  detail: Schema.optional(Schema.String),
  metrics: Schema.optional(DiagnosticMetricsSchema),
});
const LivestreamDiagnosticsSchema = Schema.Struct({
  streamerId: Schema.String,
  sessionStartedAt: Schema.optional(Schema.Number),
  stages: Schema.Record(
    Schema.Literals(["metadata", "voice", "summary", "alert"]),
    StageDiagnosticSchema,
  ),
  updatedAt: Schema.Number,
});
const LivestreamEventSchema = Schema.Struct({
  eventId: Schema.String,
  streamerId: Schema.String,
  sessionStartedAt: Schema.optional(Schema.Number),
  createdAt: Schema.Number,
  kind: Schema.Literals([
    "session",
    "metadata",
    "voice",
    "summary",
    "alert",
    "feedback",
    "anomaly",
  ]),
  status: Schema.Literals(["info", "success", "warning", "error"]),
  title: Schema.String,
  detail: Schema.optional(Schema.String),
  durationMs: Schema.optional(Schema.Number),
  costCents: Schema.optional(Schema.Number),
  metrics: Schema.optional(DiagnosticMetricsSchema),
});
const LivestreamRuntimeSchema = Schema.Struct({
  enabled: Schema.Literal(true),
  voiceprintLoaded: Schema.Boolean,
  model: Schema.String,
  queues: Schema.Struct({
    capture: Schema.Struct({ running: Schema.Number, queued: Schema.Number }),
    speech: Schema.Struct({ running: Schema.Number, queued: Schema.Number }),
    llm: Schema.Struct({ running: Schema.Number, queued: Schema.Number }),
  }),
  activeStreamCount: Schema.Number,
  activeVoiceTargetCount: Schema.Number,
  budget: Schema.Struct({
    spentCents: Schema.Number,
    limitCents: Schema.Number,
    remainingCents: Schema.Number,
  }),
  intervals: Schema.Struct({
    voiceSeconds: Schema.Number,
    summarySeconds: Schema.Number,
  }),
});
export const LivestreamIntelligenceDetailsSchema =
  browserSchema<LivestreamIntelligenceDetails>(
    Schema.Struct({
      intelligence: Schema.NullOr(LivestreamIntelligenceSchema),
      diagnostics: Schema.NullOr(LivestreamDiagnosticsSchema),
      events: Schema.Array(LivestreamEventSchema),
      runtime: Schema.NullOr(LivestreamRuntimeSchema),
      generatedAt: Schema.Number,
    }),
  );

const ShortlistScoresSchema = Schema.Struct({
  tasteMatch: Schema.Number,
  novelty: Schema.Number,
  effortFit: Schema.Number,
  composite: Schema.Number,
  risks: Schema.Array(Schema.String),
});
export const RecommendationSchema = browserSchema<Recommendation>(
  Schema.Struct({
    recommendationId: Schema.String,
    canonicalId: Schema.String,
    tmdbId: Schema.Number,
    mediaType: Schema.Literals(["movie", "tv"]),
    title: Schema.String,
    year: Schema.NullOr(Schema.Number),
    posterPath: Schema.NullOr(Schema.String),
    status: Schema.Literals([
      "pending",
      "notified",
      "watched",
      "abandoned",
      "ignored",
      "failed",
    ]),
    whyForUser: Schema.NullOr(Schema.String),
    caveats: Schema.Array(Schema.String),
    runDate: Schema.String,
    recommendedAt: Schema.Number,
    notifiedAt: Schema.NullOr(Schema.Number),
    startedAt: Schema.NullOr(Schema.Number),
    resolvedAt: Schema.NullOr(Schema.Number),
    watchlistResult: Schema.NullOr(
      Schema.Literals(["added", "already_exists", "available", "error"]),
    ),
    confidence: Schema.NullOr(Schema.Number),
    feedback: Schema.NullOr(
      Schema.Literals(["good_pick", "not_for_me", "already_watched"]),
    ),
    feedbackAt: Schema.NullOr(Schema.Number),
    feedbackNote: Schema.NullOr(Schema.String),
    source: Schema.NullOr(Schema.String),
    genres: Schema.Array(Schema.String),
    runtimeMinutes: Schema.NullOr(Schema.Number),
    seasonCount: Schema.NullOr(Schema.Number),
    episodeCount: Schema.NullOr(Schema.Number),
    seriesStatus: Schema.NullOr(Schema.String),
    originalLanguage: Schema.NullOr(Schema.String),
    originCountries: Schema.Array(Schema.String),
    creators: Schema.Array(Schema.String),
    cast: Schema.Array(Schema.String),
    keywords: Schema.Array(Schema.String),
    certification: Schema.NullOr(Schema.String),
    shortlistScores: Schema.NullOr(ShortlistScoresSchema),
    links: Schema.Struct({
      tmdb: Schema.String,
      plex: Schema.String,
      manager: Schema.String,
    }),
  }),
);

const TasteClaimSchema = Schema.Struct({
  claim: Schema.String,
  confidence: Schema.Number,
  evidenceIds: Schema.Array(Schema.String),
});
const RecommendationOutcomeStatsSchema = Schema.Struct({
  total: Schema.Number,
  watched: Schema.Number,
  abandoned: Schema.Number,
  ignored: Schema.Number,
  failed: Schema.Number,
  awaitingOutcome: Schema.Number,
});
const TasteBehaviorStatsSchema = Schema.Struct({
  completedMovies: Schema.Number,
  completedSeries: Schema.Number,
  rewatchedTitles: Schema.Number,
  recommendations: RecommendationOutcomeStatsSchema,
  feedback: Schema.Struct({
    goodPick: Schema.Number,
    notForMe: Schema.Number,
    alreadyWatched: Schema.Number,
  }),
  averageHoursToStart: Schema.optional(Schema.Number),
  sourcePerformance: Schema.Record(
    Schema.String,
    Schema.Struct({
      total: Schema.Number,
      watched: Schema.Number,
      goodPick: Schema.Number,
      notForMe: Schema.Number,
    }),
  ),
});
const TasteCommitmentSchema = Schema.Struct({
  preference: Schema.String,
  confidence: Schema.Number,
  evidenceIds: Schema.Array(Schema.String),
});
export const TasteProfileSchema = browserSchema<TasteProfile>(
  Schema.Struct({
    profileId: Schema.String,
    version: Schema.Number,
    generatedAt: Schema.Number,
    summary: Schema.String,
    stablePreferences: Schema.Array(TasteClaimSchema),
    conditionalPreferences: Schema.Array(TasteClaimSchema),
    aversions: Schema.Array(TasteClaimSchema),
    currentSaturation: Schema.Array(TasteClaimSchema),
    explorationTargets: Schema.Array(TasteClaimSchema),
    uncertainties: Schema.Array(TasteClaimSchema),
    commitmentPreferences: Schema.Struct({
      movies: TasteCommitmentSchema,
      limitedSeries: TasteCommitmentSchema,
      longSeries: TasteCommitmentSchema,
    }),
    stats: TasteBehaviorStatsSchema,
  }),
);

const PodcastShortlistScoresSchema = Schema.Struct({
  tasteMatch: Schema.Number,
  novelty: Schema.Number,
  composite: Schema.Number,
  risks: Schema.Array(Schema.String),
});
export const PodcastRecommendationSchema = browserSchema<PodcastRecommendation>(
  Schema.Struct({
    recommendationId: Schema.String,
    showTitle: Schema.String,
    episodeTitle: Schema.String,
    feedUrl: Schema.String,
    itunesId: Schema.NullOr(Schema.Number),
    artworkUrl: Schema.NullOr(Schema.String),
    episodeUrl: Schema.NullOr(Schema.String),
    publishedAt: Schema.Number,
    durationMinutes: Schema.NullOr(Schema.Number),
    status: Schema.Literals([
      "pending",
      "notified",
      "listened",
      "abandoned",
      "ignored",
      "failed",
    ]),
    whyForUser: Schema.NullOr(Schema.String),
    caveats: Schema.Array(Schema.String),
    confidence: Schema.NullOr(Schema.Number),
    shortlistScores: Schema.NullOr(PodcastShortlistScoresSchema),
    discoveredVia: Schema.NullOr(Schema.String),
    sourceUrl: Schema.NullOr(Schema.String),
    matchedVoices: Schema.Array(Schema.String),
    recommendedAt: Schema.Number,
    notifiedAt: Schema.NullOr(Schema.Number),
    queueResult: Schema.NullOr(
      Schema.Literals(["queued", "already_queued", "not_queued"]),
    ),
    feedback: Schema.NullOr(Schema.Literals(["good_pick", "not_for_me"])),
    feedbackAt: Schema.NullOr(Schema.Number),
    feedbackNote: Schema.NullOr(Schema.String),
  }),
);
export const PodcastTasteProfileSchema = browserSchema<PodcastTasteProfile>(
  Schema.Struct({
    profileId: Schema.String,
    version: Schema.Number,
    generatedAt: Schema.Number,
    summary: Schema.String,
    stablePreferences: Schema.Array(TasteClaimSchema),
    conditionalPreferences: Schema.Array(TasteClaimSchema),
    aversions: Schema.Array(TasteClaimSchema),
    currentSaturation: Schema.Array(TasteClaimSchema),
    explorationTargets: Schema.Array(TasteClaimSchema),
    uncertainties: Schema.Array(TasteClaimSchema),
    stats: Schema.Struct({
      listenedEpisodes: Schema.Number,
      startedEpisodes: Schema.Number,
      starredEpisodes: Schema.Number,
      distinctShows: Schema.Number,
      recommendations: Schema.Struct({
        total: Schema.Number,
        listened: Schema.Number,
        abandoned: Schema.Number,
        ignored: Schema.Number,
        failed: Schema.Number,
        awaitingOutcome: Schema.Number,
      }),
      feedback: Schema.Struct({ goodPick: Schema.Number, notForMe: Schema.Number }),
    }),
  }),
);

export const EmailActivitySchema = browserSchema<EmailActivity>(
  Schema.Struct({
    activityId: Schema.String,
    pipeline: Schema.Literals(["ParcelTracker", "CalendarEvents"]),
    emailId: Schema.String,
    subject: Schema.String,
    from: Schema.String,
    receivedAt: Schema.Number,
    processedAt: Schema.Number,
    outcome: Schema.Literals([
      "filtered",
      "skipped",
      "no_matches",
      "processed",
      "partial",
      "failed",
      "error",
    ]),
    admitReason: Schema.NullOr(Schema.String),
    admitTier: Schema.NullOr(Schema.String),
    costCents: Schema.NullOr(Schema.Number),
    detail: Schema.NullOr(Schema.String),
    items: Schema.Array(Schema.String),
  }),
);
const EmailRuleSchema = Schema.Struct({
  ruleId: Schema.String,
  pattern: Schema.String,
  scope: Schema.Literals(["parcel", "calendar", "both"]),
  verdict: Schema.Literals(["block", "allow"]),
  createdAt: Schema.Number,
});
const EmailFeedbackSchema = Schema.Struct({
  activityId: Schema.String,
  pipeline: Schema.Literals(["ParcelTracker", "CalendarEvents"]),
  emailId: Schema.String,
  subject: Schema.String,
  from: Schema.String,
  verdict: Schema.Literals(["not_relevant", "missed"]),
  note: Schema.optional(Schema.String),
  createdAt: Schema.Number,
});
const EmailBuiltinRulesSchema = Schema.Struct({
  parcel: Schema.Struct({
    blocked: Schema.Array(Schema.String),
    autoPass: Schema.Array(Schema.String),
  }),
  calendar: Schema.Struct({
    blocked: Schema.Array(Schema.String),
    autoPass: Schema.Array(Schema.String),
  }),
});
const CreateEmailRuleResultSchema = Schema.Struct({
  rule: Schema.optional(EmailRuleSchema),
  status: Schema.Literals(["created", "exists", "merged", "builtin"]),
  message: Schema.optional(Schema.String),
});

const BriefingNotificationSchema = Schema.Struct({
  title: Schema.String,
  message: Schema.String,
  url: Schema.String,
  timestamp: Schema.Number,
  runId: Schema.NullOr(Schema.String),
  costCents: Schema.NullOr(Schema.Number),
});
const BriefingHistorySchema = browserSchema<BriefingHistory>(
  Schema.Struct({
    name: Schema.String,
    notifications: Schema.Array(BriefingNotificationSchema),
  }),
);

const PressPodsRetrieverAttemptSchema = Schema.Union([
  Schema.Struct({
    name: Schema.String,
    success: Schema.Literal(true),
    contentRating: Schema.Number,
    textChars: Schema.Number,
  }),
  Schema.Struct({
    name: Schema.String,
    success: Schema.Literal(false),
    error: Schema.String,
  }),
]);
const PressPodsChapterSchema = Schema.Struct({
  startTimeSeconds: Schema.Number,
  title: Schema.String,
});
const PressPodsChunkStatSchema = Schema.Struct({
  index: Schema.Number,
  sectionIndex: Schema.Number,
  sectionTitle: Schema.optional(Schema.String),
  text: Schema.String,
  charCount: Schema.Number,
  durationSeconds: Schema.Number,
  startTimeSeconds: Schema.Number,
  secPerChar: Schema.Number,
  attempts: Schema.Number,
  coverage: Schema.optional(Schema.Number),
  wordRatio: Schema.optional(Schema.Number),
  expectedWords: Schema.optional(Schema.Number),
  resplit: Schema.optional(Schema.Boolean),
  resplitDepth: Schema.optional(Schema.Number),
});
const PressPodsCostsSchema = Schema.Struct({
  llmCents: Schema.Number,
  ttsCents: Schema.Number,
  detailCents: Schema.Record(Schema.String, Schema.Number),
  detailTokens: Schema.Record(
    Schema.String,
    Schema.Struct({ input: Schema.Number, output: Schema.Number }),
  ),
  detailChars: Schema.Record(Schema.String, Schema.Number),
});
const PressPodsEpisodeFields = {
  episodeId: Schema.String,
  title: Schema.String,
  author: Schema.NullOr(Schema.String),
  publication: Schema.NullOr(Schema.String),
  domain: Schema.NullOr(Schema.String),
  articleUrl: Schema.String,
  leadImageUrl: Schema.NullOr(Schema.String),
  excerpt: Schema.NullOr(Schema.String),
  voiceName: Schema.NullOr(Schema.String),
  synthesizedSeconds: Schema.NullOr(Schema.Number),
  audioUrl: Schema.String,
  durationSeconds: Schema.NullOr(Schema.Number),
  fileBytes: Schema.Number,
  retrieverName: Schema.NullOr(Schema.String),
  retrieverSeconds: Schema.NullOr(Schema.Number),
  retrieverAttempts: Schema.NullOr(Schema.Array(PressPodsRetrieverAttemptSchema)),
  chapters: Schema.NullOr(Schema.Array(PressPodsChapterSchema)),
  costCents: Schema.NullOr(Schema.Number),
  createdAt: Schema.Number,
  publishedAt: Schema.NullOr(Schema.Number),
  runId: Schema.NullOr(Schema.String),
} as const;
export const PressPodsEpisodeSchema = browserSchema<PressPodsEpisode>(
  Schema.Struct(PressPodsEpisodeFields),
);
export const PressPodsEpisodeDetailSchema = browserSchema<PressPodsEpisodeDetail>(
  Schema.Struct({
    ...PressPodsEpisodeFields,
    content: Schema.String,
    authorGender: Schema.NullOr(Schema.String),
    voiceProvider: Schema.NullOr(Schema.String),
    chunks: Schema.NullOr(Schema.Array(PressPodsChunkStatSchema)),
    costs: Schema.NullOr(PressPodsCostsSchema),
  }),
);
export const PressPodsJobSchema = browserSchema<PressPodsJob>(
  Schema.Struct({
    jobId: Schema.String,
    url: Schema.String,
    status: Schema.Literals(["queued", "processing", "failed"]),
    attempts: Schema.Number,
    nextAttemptAt: Schema.NullOr(Schema.Number),
    lastError: Schema.NullOr(Schema.String),
    createdAt: Schema.Number,
    updatedAt: Schema.Number,
    lastRunId: Schema.NullOr(Schema.String),
  }),
);

const WorkspaceArtifactDefinitionSchema = Schema.Struct({
  key: Schema.String,
  title: Schema.String,
  kind: Schema.String,
  instructions: Schema.String,
});
const WorkspaceDefinitionFields = {
  id: Schema.String,
  title: Schema.String,
  description: Schema.String,
  subjectLabel: Schema.String,
  subjectLabelPlural: Schema.String,
  taskName: Schema.String,
  schedule: Schema.String,
  scheduledRuns: Schema.optional(Schema.Boolean),
  inputPlaceholder: Schema.optional(Schema.String),
  followUpPlaceholder: Schema.optional(Schema.String),
  instructions: Schema.String,
  artifacts: Schema.Array(WorkspaceArtifactDefinitionSchema),
} as const;
export const WorkspaceDefinitionSchema = browserSchema<WorkspaceDefinition>(
  Schema.Struct(WorkspaceDefinitionFields),
);
export const WorkspaceSubjectSchema = browserSchema<WorkspaceSubject>(
  Schema.Struct({
    workspaceId: Schema.String,
    subjectId: Schema.String,
    title: Schema.String,
    status: Schema.Literals(["active", "paused", "completed", "archived"]),
    summary: Schema.String,
    createdAt: Schema.Number,
    updatedAt: Schema.Number,
    lastResearchedAt: Schema.optional(Schema.Number),
  }),
);
const WorkspaceActionSchema = Schema.Struct({
  actionId: Schema.String,
  type: Schema.Literals(["email_scope", "calendar_event"]),
  status: Schema.Literals(["pending", "approved", "rejected", "failed"]),
  title: Schema.String,
  description: Schema.String,
  payload: Schema.String,
  createdAt: Schema.Number,
  result: Schema.optional(Schema.String),
});
const WorkspacePapercutSchema = Schema.Struct({
  papercutId: Schema.String,
  category: Schema.String,
  title: Schema.String,
  detail: Schema.String,
  occurrences: Schema.Number,
  lastSeenAt: Schema.Number,
  status: Schema.Literals(["open", "addressed", "dismissed"]),
});
const WorkspaceArtifactSchema = Schema.Struct({
  revisionId: Schema.String,
  workspaceId: Schema.String,
  subjectId: Schema.String,
  artifactKey: Schema.String,
  kind: Schema.String,
  content: Schema.String,
  summary: Schema.String,
  createdAt: Schema.Number,
  runId: Schema.optional(Schema.String),
});
const WorkspaceMessageSchema = Schema.Struct({
  messageId: Schema.String,
  role: Schema.Literals(["user", "assistant", "system"]),
  text: Schema.String,
  createdAt: Schema.Number,
  runId: Schema.optional(Schema.String),
});
const WorkspaceSourceSchema = Schema.Struct({
  sourceId: Schema.String,
  kind: Schema.Literals(["web", "email"]),
  title: Schema.String,
  url: Schema.optional(Schema.String),
  excerpt: Schema.String,
  createdAt: Schema.Number,
});
const WorkspaceSummarySchema = Schema.Struct({
  ...WorkspaceDefinitionFields,
  subjects: Schema.Array(WorkspaceSubjectSchema),
  activeSubjectCount: Schema.Number,
  pendingActionCount: Schema.Number,
  openPapercutCount: Schema.Number,
});
export const WorkspaceDetailResponseSchema = browserSchema<WorkspaceDetailResponse>(
  Schema.Struct({
    workspace: WorkspaceDefinitionSchema,
    subject: WorkspaceSubjectSchema,
    artifacts: Schema.Array(WorkspaceArtifactSchema),
    artifactRevisions: Schema.Array(WorkspaceArtifactSchema),
    messages: Schema.Array(WorkspaceMessageSchema),
    sources: Schema.Array(WorkspaceSourceSchema),
    actions: Schema.Array(WorkspaceActionSchema),
    emailScope: Schema.NullOr(
      Schema.Struct({
        senders: Schema.Array(Schema.String),
        domains: Schema.Array(Schema.String),
        subjectKeywords: Schema.Array(Schema.String),
        bodyKeywords: Schema.Array(Schema.String),
      }),
    ),
    papercuts: Schema.Array(WorkspacePapercutSchema),
  }),
);

const DataEntitySchema = Schema.Struct({
  slug: Schema.String,
  label: Schema.String,
  description: Schema.String,
  warning: Schema.optional(Schema.String),
  primaryKey: Schema.Array(Schema.String),
  count: Schema.Number,
  storageBytes: Schema.Number,
});
const DataStorageSummarySchema = Schema.Struct({
  databaseSizeBytes: Schema.Number,
  entityStorageBytes: Schema.Number,
});
// Entity row columns are selected dynamically at runtime. Values are intentionally
// open here, while the response envelope and entity metadata remain strict.
const DataRowSchema = Schema.Record(Schema.String, Schema.Unknown);

const HighestDaySchema = Schema.Struct({
  date: Schema.String,
  costCents: Schema.Number,
});
const CostSummarySchema = Schema.Struct({
  selectedCostCents: Schema.Number,
  allTimeCostCents: Schema.Number,
  allTimeUnknownEventCount: Schema.Number,
  averageDailyCostCents: Schema.Number,
  highestDay: Schema.NullOr(HighestDaySchema),
  eventCount: Schema.Number,
  unknownEventCount: Schema.Number,
  inputTokens: Schema.Number,
  outputTokens: Schema.Number,
  characters: Schema.Number,
  requests: Schema.Number,
  credits: Schema.Number,
});
const DailyCostSchema = Schema.Struct({
  date: Schema.String,
  costCents: Schema.Number,
  byFeature: Schema.Record(Schema.String, Schema.Number),
  pricedEventCount: Schema.Number,
  unknownEventCount: Schema.Number,
});
const FeatureCostSchema = Schema.Struct({
  feature: Schema.String,
  costCents: Schema.Number,
  eventCount: Schema.Number,
  unknownEventCount: Schema.Number,
});
const ServiceCostSchema = Schema.Struct({
  service: Schema.String,
  model: Schema.NullOr(Schema.String),
  category: Schema.String,
  costCents: Schema.Number,
  eventCount: Schema.Number,
  unknownEventCount: Schema.Number,
  inputTokens: Schema.Number,
  outputTokens: Schema.Number,
  characters: Schema.Number,
  requests: Schema.Number,
  credits: Schema.Number,
});
const CostEventSchema = Schema.Struct({
  eventId: Schema.String,
  incurredAt: Schema.Number,
  feature: Schema.String,
  operation: Schema.String,
  service: Schema.String,
  model: Schema.NullOr(Schema.String),
  category: Schema.String,
  costCents: Schema.NullOr(Schema.Number),
  priceStatus: Schema.String,
  runId: Schema.NullOr(Schema.String),
});
const CostsResponseSchema = Schema.Struct({
  range: Schema.Struct({
    days: Schema.NullOr(Schema.Number),
    from: Schema.NullOr(Schema.Number),
    to: Schema.Number,
  }),
  summary: CostSummarySchema,
  daily: Schema.Array(DailyCostSchema),
  byFeature: Schema.Array(FeatureCostSchema),
  byService: Schema.Array(ServiceCostSchema),
  recent: Schema.Array(CostEventSchema),
});

export function fetchTasks(): Effect.Effect<{ tasks: TaskInfo[] }, ApiClientError> {
  return apiGet("/api/tasks", listResponseSchema("tasks", TaskInfoSchema));
}

export function fetchSnapshot(): Effect.Effect<Snapshot, ApiClientError> {
  return apiGet("/api/snapshot", SnapshotSchema);
}

export function fetchDataEntities(): Effect.Effect<
  {
    entities: DataEntity[];
    storage: DataStorageSummary;
  },
  ApiClientError
> {
  return apiGet(
    "/api/data/entities",
    browserSchema<{
      entities: DataEntity[];
      storage: DataStorageSummary;
    }>(
      Schema.Struct({
        entities: Schema.Array(DataEntitySchema),
        storage: DataStorageSummarySchema,
      }),
    ),
  );
}

export function fetchDataRows(
  slug: string,
): Effect.Effect<{ summary: DataEntity; rows: DataRow[] }, ApiClientError> {
  return apiGet(
    `/api/data/entities/${encodeURIComponent(slug)}`,
    browserSchema<{ summary: DataEntity; rows: DataRow[] }>(
      Schema.Struct({ summary: DataEntitySchema, rows: Schema.Array(DataRowSchema) }),
    ),
  );
}

export function deleteDataRow(
  slug: string,
  key: DataRow,
): Effect.Effect<{ deleted: true }, ApiClientError> {
  return apiDelete(
    `/api/data/entities/${encodeURIComponent(slug)}`,
    DeletedTrueResponse,
    { key },
  );
}

export function fetchTaskRuns(options?: {
  task?: string;
  limit?: number;
}): Effect.Effect<{ runs: TaskRun[] }, ApiClientError> {
  const params = new URLSearchParams();
  if (options?.task) params.set("task", options.task);
  if (options?.limit !== undefined) params.set("limit", String(options.limit));
  const query = params.toString();
  return apiGet(
    `/api/task-runs${query ? `?${query}` : ""}`,
    listResponseSchema("runs", TaskRunSchema),
  );
}

export function fetchRunLogs(runId: string): Effect.Effect<RunLogs, ApiClientError> {
  return apiGet(`/api/task-runs/${encodeURIComponent(runId)}/logs`, RunLogsSchema);
}

export function runLogStreamUrl(runId: string): string {
  return `/api/task-runs/${encodeURIComponent(runId)}/logs/stream`;
}

export function runTaskRequest(
  name: string,
  options?: ManualRunOptions,
): Effect.Effect<{ runId: string }, ApiClientError> {
  if (options?.maxRecommendations !== undefined) {
    const path =
      name === "PodcastRecs"
        ? "/api/podcast-recommendations/run"
        : "/api/recommendations/run";
    return apiPost(path, RunIdResponse, {
      maxRecommendations: options.maxRecommendations,
    });
  }
  return apiPost(`/api/tasks/${encodeURIComponent(name)}/run`, RunIdResponse);
}

export function fetchStreamerMetrics(
  id: string,
): Effect.Effect<StreamerMetrics, ApiClientError> {
  return apiGet(
    `/api/streamers/${encodeURIComponent(id)}/metrics`,
    StreamerMetricsSchema,
  );
}

export function fetchStreamerSessions(
  id: string,
): Effect.Effect<{ sessions: StreamSession[] }, ApiClientError> {
  return apiGet(
    `/api/streamers/${encodeURIComponent(id)}/sessions`,
    listResponseSchema("sessions", StreamSessionSchema),
  );
}

export function fetchLivestreamIntelligenceDetails(
  id: string,
  limit = 100,
): Effect.Effect<LivestreamIntelligenceDetails, ApiClientError> {
  return apiGet(
    `/api/streamers/${encodeURIComponent(id)}/intelligence-details?limit=${limit}`,
    LivestreamIntelligenceDetailsSchema,
  );
}

export function fetchRecommendations(): Effect.Effect<
  {
    recommendations: Recommendation[];
  },
  ApiClientError
> {
  return apiGet(
    "/api/recommendations",
    listResponseSchema("recommendations", RecommendationSchema),
  );
}

export function fetchTasteProfile(): Effect.Effect<
  { profile: TasteProfile | null },
  ApiClientError
> {
  return apiGet(
    "/api/recommendations/taste-profile",
    Schema.Struct({ profile: Schema.NullOr(TasteProfileSchema) }),
  );
}

export function fetchPressPods(): Effect.Effect<
  {
    episodes: PressPodsEpisode[];
    jobs: PressPodsJob[];
  },
  ApiClientError
> {
  return apiGet(
    "/api/press-pods/episodes",
    browserSchema<{ episodes: PressPodsEpisode[]; jobs: PressPodsJob[] }>(
      Schema.Struct({
        episodes: Schema.Array(PressPodsEpisodeSchema),
        jobs: Schema.Array(PressPodsJobSchema),
      }),
    ),
  );
}

export function fetchPressPodsEpisode(
  episodeId: string,
): Effect.Effect<{ episode: PressPodsEpisodeDetail }, ApiClientError> {
  return apiGet(
    `/api/press-pods/episodes/${encodeURIComponent(episodeId)}`,
    itemResponseSchema("episode", PressPodsEpisodeDetailSchema),
  );
}

export function submitPressPodsUrl(
  url: string,
): Effect.Effect<{ job: PressPodsJob }, ApiClientError> {
  return apiPost(
    "/api/press-pods/submit",
    itemResponseSchema("job", PressPodsJobSchema),
    {
      url,
    },
  );
}

export function retryPressPodsJob(
  jobId: string,
): Effect.Effect<{ job: PressPodsJob }, ApiClientError> {
  return apiPost(
    `/api/press-pods/jobs/${encodeURIComponent(jobId)}/retry`,
    itemResponseSchema("job", PressPodsJobSchema),
  );
}

export function dismissPressPodsJob(
  jobId: string,
): Effect.Effect<{ deleted: boolean }, ApiClientError> {
  return apiDelete(
    `/api/press-pods/jobs/${encodeURIComponent(jobId)}`,
    DeletedBooleanResponse,
  );
}

export function deletePressPodsEpisode(
  episodeId: string,
): Effect.Effect<{ deleted: boolean }, ApiClientError> {
  return apiDelete(
    `/api/press-pods/episodes/${encodeURIComponent(episodeId)}`,
    DeletedBooleanResponse,
  );
}

export function retryPressPodsEpisode(
  episodeId: string,
): Effect.Effect<{ job: PressPodsJob }, ApiClientError> {
  return apiPost(
    `/api/press-pods/episodes/${encodeURIComponent(episodeId)}/retry`,
    itemResponseSchema("job", PressPodsJobSchema),
  );
}

export function fetchBriefings(): Effect.Effect<
  { briefings: BriefingHistory[] },
  ApiClientError
> {
  return apiGet(
    "/api/briefings",
    listResponseSchema("briefings", BriefingHistorySchema),
  );
}

export function fetchWorkspaces(): Effect.Effect<
  { workspaces: WorkspaceSummary[] },
  ApiClientError
> {
  return apiGet(
    "/api/workspaces",
    listResponseSchema(
      "workspaces",
      browserSchema<WorkspaceSummary>(WorkspaceSummarySchema),
    ),
  );
}

export function fetchWorkspace(workspaceId: string): Effect.Effect<
  {
    workspace: WorkspaceDefinition;
    subjects: WorkspaceSubject[];
    actions: WorkspaceAction[];
    papercuts: WorkspacePapercut[];
  },
  ApiClientError
> {
  return apiGet(
    `/api/workspaces/${encodeURIComponent(workspaceId)}`,
    browserSchema<{
      workspace: WorkspaceDefinition;
      subjects: WorkspaceSubject[];
      actions: WorkspaceAction[];
      papercuts: WorkspacePapercut[];
    }>(
      Schema.Struct({
        workspace: WorkspaceDefinitionSchema,
        subjects: Schema.Array(WorkspaceSubjectSchema),
        actions: Schema.Array(WorkspaceActionSchema),
        papercuts: Schema.Array(WorkspacePapercutSchema),
      }),
    ),
  );
}

export function fetchWorkspaceSubject(
  workspaceId: string,
  subjectId: string,
): Effect.Effect<WorkspaceDetailResponse, ApiClientError> {
  return apiGet(
    `/api/workspaces/${encodeURIComponent(workspaceId)}/subjects/${encodeURIComponent(subjectId)}`,
    WorkspaceDetailResponseSchema,
  );
}

export function sendWorkspaceMessage(
  workspaceId: string,
  message: string,
  subjectId?: string,
): Effect.Effect<{ runId: string }, ApiClientError> {
  return apiPost(
    `/api/workspaces/${encodeURIComponent(workspaceId)}/messages`,
    RunIdResponse,
    {
      message,
      subjectId,
    },
  );
}

export function setWorkspaceSubjectStatus(
  workspaceId: string,
  subjectId: string,
  status: WorkspaceSubjectStatus,
): Effect.Effect<{ subject: WorkspaceSubject }, ApiClientError> {
  return apiPost(
    `/api/workspaces/${encodeURIComponent(workspaceId)}/subjects/${encodeURIComponent(subjectId)}/status`,
    itemResponseSchema("subject", WorkspaceSubjectSchema),
    { status },
  );
}

export function resolveWorkspaceAction(
  actionId: string,
  resolution: "approve" | "reject",
): Effect.Effect<{ action: WorkspaceAction }, ApiClientError> {
  return apiPost(
    `/api/workspace-actions/${encodeURIComponent(actionId)}/${resolution}`,
    itemResponseSchema("action", browserSchema<WorkspaceAction>(WorkspaceActionSchema)),
  );
}

export function fetchCosts(
  range: CostRange,
): Effect.Effect<CostsResponse, ApiClientError> {
  return apiGet(
    `/api/costs?days=${range}`,
    browserSchema<CostsResponse>(CostsResponseSchema),
  );
}

export function submitLivestreamFeedback(
  streamerId: string,
  alertId: string,
  verdict: "useful" | "not_useful" | "false_positive",
  note?: string,
): Effect.Effect<{ feedback: LivestreamFeedback }, ApiClientError> {
  return apiPost(
    `/api/streamers/${encodeURIComponent(streamerId)}/intelligence-feedback`,
    itemResponseSchema(
      "feedback",
      browserSchema<LivestreamFeedback>(LivestreamFeedbackSchema),
    ),
    { alertId, verdict, note },
  );
}

export function fetchEmailActivity(
  pipeline?: EmailPipeline,
  limit?: number,
): Effect.Effect<{ activities: EmailActivity[] }, ApiClientError> {
  const params = new URLSearchParams();
  if (pipeline) params.set("pipeline", pipeline);
  if (limit !== undefined) params.set("limit", String(limit));
  const query = params.toString();
  return apiGet(
    `/api/email-activity${query ? `?${query}` : ""}`,
    listResponseSchema("activities", EmailActivitySchema),
  );
}

export interface EmailActivityLogs {
  activity: EmailActivity;
  lines: RunLogLine[];
  dropped: number;
}

export function fetchEmailActivityLogs(
  activityId: string,
): Effect.Effect<EmailActivityLogs, ApiClientError> {
  return apiGet(
    `/api/email-activity/${encodeURIComponent(activityId)}/logs`,
    browserSchema<EmailActivityLogs>(
      Schema.Struct({
        activity: EmailActivitySchema,
        lines: Schema.Array(RunLogLineSchema),
        dropped: Schema.Number,
      }),
    ),
  );
}

export interface EmailBuiltinRules {
  parcel: { blocked: string[]; autoPass: string[] };
  calendar: { blocked: string[]; autoPass: string[] };
}

export function fetchEmailRules(): Effect.Effect<
  {
    rules: EmailRule[];
    builtin: EmailBuiltinRules;
  },
  ApiClientError
> {
  return apiGet(
    "/api/email-rules",
    browserSchema<{ rules: EmailRule[]; builtin: EmailBuiltinRules }>(
      Schema.Struct({
        rules: Schema.Array(EmailRuleSchema),
        builtin: EmailBuiltinRulesSchema,
      }),
    ),
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
}): Effect.Effect<CreateEmailRuleResult, ApiClientError> {
  return apiPost("/api/email-rules", CreateEmailRuleResultSchema, input);
}

export function deleteEmailRule(
  ruleId: string,
): Effect.Effect<{ deleted: true }, ApiClientError> {
  return apiDelete(
    `/api/email-rules/${encodeURIComponent(ruleId)}`,
    DeletedTrueResponse,
  );
}

export function fetchEmailFeedback(): Effect.Effect<
  { feedback: EmailFeedback[] },
  ApiClientError
> {
  return apiGet(
    "/api/email-feedback",
    listResponseSchema("feedback", browserSchema<EmailFeedback>(EmailFeedbackSchema)),
  );
}

export function sendEmailActivityFeedback(
  activityId: string,
  verdict: EmailFeedbackVerdict | null,
  note?: string,
): Effect.Effect<{ feedback: EmailFeedback | null }, ApiClientError> {
  return apiPost(
    `/api/email-activity/${encodeURIComponent(activityId)}/feedback`,
    Schema.Struct({ feedback: Schema.NullOr(EmailFeedbackSchema) }),
    { verdict, ...(note === undefined ? {} : { note }) },
  );
}

export function reprocessEmailActivity(
  activityId: string,
): Effect.Effect<{ activity: EmailActivity }, ApiClientError> {
  return apiPost(
    `/api/email-activity/${encodeURIComponent(activityId)}/reprocess`,
    itemResponseSchema("activity", EmailActivitySchema),
  );
}

export function forgetParcelDelivery(
  trackingNumber: string,
): Effect.Effect<{ deleted: true }, ApiClientError> {
  return apiDelete(
    `/api/parcel-tracker/deliveries/${encodeURIComponent(trackingNumber)}`,
    DeletedTrueResponse,
  );
}

export function fetchRecommendation(
  recommendationId: string,
): Effect.Effect<{ recommendation: Recommendation }, ApiClientError> {
  return apiGet(
    `/api/recommendations/${encodeURIComponent(recommendationId)}`,
    itemResponseSchema("recommendation", RecommendationSchema),
  );
}

export function fetchPodcastRecommendation(
  recommendationId: string,
): Effect.Effect<{ recommendation: PodcastRecommendation }, ApiClientError> {
  return apiGet(
    `/api/podcast-recommendations/${encodeURIComponent(recommendationId)}`,
    itemResponseSchema("recommendation", PodcastRecommendationSchema),
  );
}

export function sendRecommendationFeedback(
  recommendationId: string,
  feedback: RecommendationFeedback | null,
  note?: string,
): Effect.Effect<{ recommendation: Recommendation }, ApiClientError> {
  return apiPost(
    `/api/recommendations/${encodeURIComponent(recommendationId)}/feedback`,
    itemResponseSchema("recommendation", RecommendationSchema),
    {
      ...(feedback ? { feedback } : {}),
      ...(note === undefined ? {} : { note }),
    },
  );
}

export function fetchPodcastRecommendations(): Effect.Effect<
  {
    recommendations: PodcastRecommendation[];
  },
  ApiClientError
> {
  return apiGet(
    "/api/podcast-recommendations",
    listResponseSchema("recommendations", PodcastRecommendationSchema),
  );
}

export function fetchPodcastTasteProfile(): Effect.Effect<
  {
    profile: PodcastTasteProfile | null;
  },
  ApiClientError
> {
  return apiGet(
    "/api/podcast-recommendations/taste-profile",
    Schema.Struct({ profile: Schema.NullOr(PodcastTasteProfileSchema) }),
  );
}

export function sendPodcastRecommendationFeedback(
  recommendationId: string,
  feedback: PodcastFeedback | null,
  note?: string,
): Effect.Effect<{ recommendation: PodcastRecommendation }, ApiClientError> {
  return apiPost(
    `/api/podcast-recommendations/${encodeURIComponent(recommendationId)}/feedback`,
    itemResponseSchema("recommendation", PodcastRecommendationSchema),
    {
      ...(feedback ? { feedback } : {}),
      ...(note === undefined ? {} : { note }),
    },
  );
}
