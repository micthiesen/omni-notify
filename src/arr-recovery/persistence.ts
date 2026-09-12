import { Entity } from "@micthiesen/mitools/entities";
import { decodeDoc, Docstore } from "@micthiesen/mitools/docstore";
import { Clock, Effect, Schema } from "effect";
import type { ArrKind, Decision, ImportFile, Target } from "./types.js";
import { ArrRecoveryError } from "./types.js";

export interface Observation {
  fingerprint: string;
  firstSeenAt: number;
  lastSeenAt: number;
  observations: number;
  lastAssessedAt?: number;
  reason?: string;
}

export interface RecoveryAction {
  downloadId: string;
  title: string;
  target: Target;
  files: ImportFile[];
  outputPath: string;
  decision: Decision;
  phase: "reserved" | "submitted" | "removed" | "searching" | "done" | "uncertain";
  createdAt: number;
  updatedAt: number;
  commandId?: number;
  error?: string;
  notification: "pending" | "sending" | "sent";
}

export interface RecoveryState {
  kind: ArrKind;
  observations: Record<string, Observation>;
  actions: RecoveryAction[];
  lease?: { owner: string; expiresAt: number };
}

const ObservationSchema = Schema.Struct({
  fingerprint: Schema.String,
  firstSeenAt: Schema.Number,
  lastSeenAt: Schema.Number,
  observations: Schema.Number,
  lastAssessedAt: Schema.optional(Schema.Number),
  reason: Schema.optional(Schema.String),
});

const TargetSchema = Schema.Struct({
  id: Schema.Number,
  title: Schema.String,
  year: Schema.Number,
  monitored: Schema.Boolean,
  hasFile: Schema.Boolean,
  path: Schema.String,
  episodeIds: Schema.mutable(Schema.Array(Schema.Number)),
  episodes: Schema.mutable(
    Schema.Array(
      Schema.Struct({
        id: Schema.Number,
        seasonNumber: Schema.Number,
        episodeNumber: Schema.Number,
        title: Schema.String,
        hasFile: Schema.Boolean,
        monitored: Schema.Boolean,
      }),
    ),
  ),
  alternateTitles: Schema.mutable(Schema.Array(Schema.String)),
});

const ImportFileSchema = Schema.Struct({
  folderName: Schema.optional(Schema.String),
  id: Schema.Number,
  path: Schema.String,
  name: Schema.String,
  size: Schema.Number,
  seriesId: Schema.optional(Schema.Number),
  movieId: Schema.optional(Schema.Number),
  seasonNumber: Schema.optional(Schema.Number),
  episodeIds: Schema.mutable(Schema.Array(Schema.Number)),
  quality: Schema.Record(Schema.String, Schema.Unknown),
  languages: Schema.optional(
    Schema.mutable(
      Schema.Array(Schema.Struct({ id: Schema.Number, name: Schema.String })),
    ),
  ),
  releaseGroup: Schema.optional(Schema.String),
  indexerFlags: Schema.optional(Schema.Number),
  releaseType: Schema.optional(Schema.String),
  rejections: Schema.mutable(
    Schema.Array(Schema.Struct({ reason: Schema.String, type: Schema.String })),
  ),
});

const DecisionSchema = Schema.Union([
  Schema.Struct({
    action: Schema.Literal("import"),
    reason: Schema.String,
    source: Schema.Literals(["rules", "llm"]),
  }),
  Schema.Struct({
    action: Schema.Literal("remove"),
    reason: Schema.String,
    source: Schema.Literals(["rules", "llm"]),
    replace: Schema.Boolean,
  }),
  Schema.Struct({
    action: Schema.Literal("defer"),
    reason: Schema.String,
    source: Schema.Literals(["rules", "llm"]),
  }),
]);

const RecoveryActionSchema = Schema.Struct({
  downloadId: Schema.String,
  title: Schema.String,
  target: TargetSchema,
  files: Schema.mutable(Schema.Array(ImportFileSchema)),
  outputPath: Schema.String,
  decision: DecisionSchema,
  phase: Schema.Literals([
    "reserved",
    "submitted",
    "removed",
    "searching",
    "done",
    "uncertain",
  ]),
  createdAt: Schema.Number,
  updatedAt: Schema.Number,
  commandId: Schema.optional(Schema.Number),
  error: Schema.optional(Schema.String),
  notification: Schema.Literals(["pending", "sending", "sent"]),
});

export const RecoveryStateSchema = Schema.Struct({
  kind: Schema.Literals(["sonarr", "radarr"]),
  observations: Schema.Record(Schema.String, ObservationSchema),
  actions: Schema.mutable(Schema.Array(RecoveryActionSchema)),
  lease: Schema.optional(
    Schema.Struct({ owner: Schema.String, expiresAt: Schema.Number }),
  ),
});

export const RecoveryStateEntity = new Entity<RecoveryState, ["kind"]>(
  "arr-recovery-state",
  ["kind"],
);

export const RECOVERY_LEASE_MS = 25 * 60 * 1_000;

const fail = (operation: string) => (cause: unknown) =>
  new ArrRecoveryError({ operation, cause });

const decodeState = (data: Buffer): RecoveryState =>
  Schema.decodeUnknownSync(RecoveryStateSchema)(decodeDoc(data));

export function acquireState(kind: ArrKind, owner: string, now: number) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const pk = RecoveryStateEntity.getPk({ kind });
    return yield* docstore.transaction("acquire Arr recovery lease", (tx) => {
      const raw = tx.getRawRow(pk, now);
      const current = raw
        ? decodeState(raw.data)
        : { kind, observations: {}, actions: [] };
      if (current.kind !== kind) {
        throw new Error(`Arr recovery state kind mismatch for ${kind}`);
      }
      if (current.lease && current.lease.expiresAt > now) return undefined;

      const next: RecoveryState = {
        ...current,
        lease: { owner, expiresAt: now + RECOVERY_LEASE_MS },
      };
      tx.upsertDoc(pk, next, { entity: RecoveryStateEntity.name }, now);
      return next;
    });
  }).pipe(Effect.mapError(fail(`acquire ${kind} recovery state`)));
}

export function saveState(state: RecoveryState, owner: string, now: number) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const pk = RecoveryStateEntity.getPk({ kind: state.kind });
    yield* docstore.transaction("save Arr recovery state", (tx) => {
      const raw = tx.getRawRow(pk, now);
      if (!raw) throw new Error(`No recovery state exists for ${state.kind}`);
      const current = decodeState(raw.data);
      if (current.kind !== state.kind) {
        throw new Error(`Arr recovery state kind mismatch for ${state.kind}`);
      }
      if (current.lease?.owner !== owner) {
        throw new Error(`Arr recovery state lease is not owned by ${owner}`);
      }
      if (current.lease.expiresAt <= now) {
        throw new Error(`Arr recovery state lease owned by ${owner} has expired`);
      }

      const next: RecoveryState = { ...state, lease: current.lease };
      Schema.decodeUnknownSync(RecoveryStateSchema)(next);
      tx.upsertDoc(pk, next, { entity: RecoveryStateEntity.name }, now);
    });
  }).pipe(Effect.mapError(fail(`save ${state.kind} recovery state`)));
}

export function releaseState(kind: ArrKind, owner: string) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const now = yield* Clock.currentTimeMillis;
    const pk = RecoveryStateEntity.getPk({ kind });
    yield* docstore.transaction("release Arr recovery lease", (tx) => {
      const raw = tx.getRawRow(pk, now);
      if (!raw) return;
      const current = decodeState(raw.data);
      if (current.kind !== kind) {
        throw new Error(`Arr recovery state kind mismatch for ${kind}`);
      }
      if (current.lease?.owner !== owner) return;

      const { lease: _lease, ...released } = current;
      tx.upsertDoc(pk, released, { entity: RecoveryStateEntity.name }, now);
    });
  }).pipe(Effect.mapError(fail(`release ${kind} recovery state`)));
}
