export type TaskTrigger = "schedule" | "manual" | "startup";
export type TaskRunStatus = "running" | "success" | "error";

export interface TaskRun {
  runId: string;
  taskName: string;
  trigger: TaskTrigger;
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

export type MediaType = "movie" | "tv";
export type RecommendationStatus =
  | "pending"
  | "notified"
  | "watched"
  | "abandoned"
  | "ignored"
  | "failed";
export type WatchlistResult = "added" | "already_exists" | "skipped" | "error";

export interface Recommendation {
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
  resolvedAt: number | null;
  watchlistResult: WatchlistResult | null;
  confidence: number | null;
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

export async function apiPost<T>(path: string): Promise<T> {
  const res = await fetch(path, { method: "POST" });
  if (!res.ok) {
    throw new ApiError(res.status, await extractErrorMessage(res));
  }
  return (await res.json()) as T;
}

export function fetchTasks(): Promise<{ tasks: TaskInfo[] }> {
  return apiGet<{ tasks: TaskInfo[] }>("/api/tasks");
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

export function runTaskRequest(name: string): Promise<{ runId: string }> {
  return apiPost<{ runId: string }>(`/api/tasks/${encodeURIComponent(name)}/run`);
}

export function fetchRecommendations(): Promise<{
  recommendations: Recommendation[];
}> {
  return apiGet<{ recommendations: Recommendation[] }>("/api/recommendations");
}
