import { Data, type Effect } from "effect";

export type ArrKind = "sonarr" | "radarr";
export class ArrRecoveryError extends Data.TaggedError("ArrRecoveryError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    return `${this.operation}: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}
export interface QueueItem {
  id: number;
  downloadId: string;
  title: string;
  status: string;
  trackedDownloadStatus: string;
  trackedDownloadState: string;
  statusMessages: { title: string; messages: string[] }[];
  size: number;
  sizeleft: number;
  outputPath?: string;
  added?: string;
  seriesId?: number;
  episodeId?: number;
  movieId?: number;
  protocol?: string;
  downloadClient?: string;
}
export interface Target {
  id: number;
  title: string;
  year: number;
  monitored: boolean;
  hasFile: boolean;
  path: string;
  episodeIds: number[];
  episodes: {
    id: number;
    seasonNumber: number;
    episodeNumber: number;
    title: string;
    hasFile: boolean;
    monitored: boolean;
  }[];
  alternateTitles: string[];
}
export interface ImportFile {
  id: number;
  path: string;
  folderName?: string;
  name: string;
  size: number;
  seriesId?: number;
  movieId?: number;
  seasonNumber?: number;
  episodeIds: number[];
  quality: Record<string, unknown>;
  languages?: { id: number; name: string }[];
  releaseGroup?: string;
  indexerFlags?: number;
  releaseType?: string;
  rejections: { reason: string; type: string }[];
}
export interface Grab {
  downloadId: string;
  sourceTitle: string;
  seriesId?: number;
  movieId?: number;
  episodeId?: number;
  eventType: string;
  date: string;
}
export interface Evidence {
  downloadHealth?: string;
  kind: ArrKind;
  items: QueueItem[];
  target: Target;
  files: ImportFile[];
  grabs: Grab[];
}
export type Decision =
  | { action: "import"; reason: string; source: "rules" | "llm" }
  | { action: "remove"; reason: string; source: "rules" | "llm"; replace: boolean }
  | { action: "defer"; reason: string; source: "rules" | "llm" };
export interface ArrClient {
  kind: ArrKind;
  queue(): Effect.Effect<QueueItem[], ArrRecoveryError>;
  preview(downloadId: string): Effect.Effect<ImportFile[], ArrRecoveryError>;
  target(items: QueueItem[]): Effect.Effect<Target, ArrRecoveryError>;
  history(downloadId: string): Effect.Effect<Grab[], ArrRecoveryError>;
  importFiles(
    downloadId: string,
    files: ImportFile[],
  ): Effect.Effect<number, ArrRecoveryError>;
  command(
    id: number,
  ): Effect.Effect<{ status: string; message?: string }, ArrRecoveryError>;
  remove(id: number, blocklist: boolean): Effect.Effect<void, ArrRecoveryError>;
  verifyRemoved(outputPath: string): Effect.Effect<boolean, ArrRecoveryError>;
  search(target: Target): Effect.Effect<number, ArrRecoveryError>;
  verifyImported(
    target: Target,
    files: ImportFile[],
  ): Effect.Effect<boolean, ArrRecoveryError>;
}
