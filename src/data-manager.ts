import { deleteDoc, getDb, getDoc } from "@micthiesen/mitools/docstore";
import type { Entity } from "@micthiesen/mitools/entities";
import { BriefingHistoryEntity } from "./briefing-agent/persistence.js";
import { CreatedCalendarEventEntity } from "./calendar-events/persistence.js";
import { EmailActivityEntity } from "./jmap/activity.js";
import { EmailStateEntity } from "./jmap/persistence.js";
import { ViewerMetricsEntity } from "./live-check/metrics/persistence.js";
import { StreamerStatusEntity } from "./live-check/persistence.js";
import { StreamSessionsEntity } from "./live-check/sessions.js";
import { SubmittedDeliveryEntity } from "./parcel-tracker/persistence.js";
import { PodcastRecommendationEntity } from "./podcast-recs/persistence.js";
import {
  PodcastTasteEvidenceEntity,
  PodcastTasteProfileEntity,
} from "./podcast-recs/reflection/index.js";
import {
  IdentityAliasEntity,
  RecommendationEntity,
} from "./recommendations/persistence.js";
import {
  TasteEvidenceEntity,
  TasteProfileEntity,
} from "./recommendations/taste/persistence.js";
import {
  TaskRunEntity,
  TaskRunLogEntity,
  TaskScheduleStateEntity,
} from "./task-runs/persistence.js";

export type DataRow = Record<string, unknown>;
export type DataRowKey = Record<string, unknown>;

export interface ManagedEntitySummary {
  slug: string;
  label: string;
  description: string;
  warning?: string;
  primaryKey: string[];
  count: number;
  /** Encoded CBOR payload bytes, excluding SQLite indexes and page overhead. */
  storageBytes: number;
}

export interface ManagedDataSummary {
  /** SQLite page allocation, including all Entity and relational table data. */
  databaseSizeBytes: number;
  /** Encoded payload bytes belonging to registered mitools Entities. */
  entityStorageBytes: number;
}

interface EntityOptions<Data> {
  label: string;
  description: string;
  warning?: string;
  canDelete?: (row: Data) => string | undefined;
  afterDelete?: (row: Data) => void;
}

interface ManagedEntity extends Omit<ManagedEntitySummary, "count" | "storageBytes"> {
  count(): number;
  storageBytes(): number;
  rows(): DataRow[];
  delete(key: DataRowKey): DeleteResult;
}

export type DeleteResult =
  | { status: "deleted"; row: DataRow }
  | { status: "invalid-key" }
  | { status: "not-found" }
  | { status: "blocked"; reason: string };

export const MALFORMED_ROW_KEY = "__dataManagerMalformed";

export type MalformedRowMetadata = {
  rawKey: string;
  error: string;
};

function decodeError(error: unknown): string {
  return error instanceof Error ? error.message : "Stored data could not be decoded";
}

function malformedRow(rawKey: string, error: unknown): DataRow {
  return {
    [MALFORMED_ROW_KEY]: {
      rawKey,
      error: decodeError(error),
    } satisfies MalformedRowMetadata,
  };
}

function getMalformedMetadata(key: DataRowKey): MalformedRowMetadata | undefined {
  if (Object.keys(key).length !== 1) return undefined;
  const value = key[MALFORMED_ROW_KEY];
  if (
    typeof value !== "object" ||
    value === null ||
    !("rawKey" in value) ||
    typeof value.rawKey !== "string" ||
    !("error" in value) ||
    typeof value.error !== "string"
  ) {
    return undefined;
  }
  return { rawKey: value.rawKey, error: value.error };
}

export function createManagedEntity<
  Data extends object,
  PKProps extends readonly (keyof Data)[],
>(entity: Entity<Data, PKProps>, options: EntityOptions<Data>): ManagedEntity {
  const primaryKey = entity.pkProps.map(String);

  const normalizeKey = (key: DataRowKey): Pick<Data, PKProps[number]> | undefined => {
    if (
      Object.keys(key).length !== primaryKey.length ||
      !primaryKey.every((property) => Object.hasOwn(key, property))
    ) {
      return undefined;
    }
    return Object.fromEntries(
      primaryKey.map((property) => [property, key[property]]),
    ) as Pick<Data, PKProps[number]>;
  };

  return {
    slug: entity.name,
    label: options.label,
    description: options.description,
    warning: options.warning,
    primaryKey,
    count: () => entity.count(),
    storageBytes: () => {
      const row = getDb()
        .prepare(
          "SELECT COALESCE(SUM(LENGTH(data)), 0) AS bytes FROM blobs WHERE pk LIKE ?",
        )
        .get(`$${entity.name}#%`) as { bytes: number };
      return row.bytes;
    },
    rows: () => {
      const rows: DataRow[] = [];
      for (const rawKey of entity.keys()) {
        try {
          const row = getDoc<Data>(rawKey);
          if (row) rows.push(row as DataRow);
        } catch (error) {
          rows.push(malformedRow(rawKey, error));
        }
      }
      return rows;
    },
    delete: (key) => {
      const malformed = getMalformedMetadata(key);
      if (malformed) {
        if (!entity.keys().includes(malformed.rawKey)) return { status: "not-found" };
        if (!deleteDoc(malformed.rawKey)) return { status: "not-found" };
        return {
          status: "deleted",
          row: malformedRow(malformed.rawKey, malformed.error),
        };
      }
      const normalized = normalizeKey(key);
      if (!normalized) return { status: "invalid-key" };
      const row = entity.get(normalized);
      if (!row) return { status: "not-found" };
      const reason = options.canDelete?.(row);
      if (reason) return { status: "blocked", reason };
      if (!entity.delete(normalized)) return { status: "not-found" };
      options.afterDelete?.(row);
      return { status: "deleted", row: row as DataRow };
    },
  };
}

const MANAGED_ENTITIES: ManagedEntity[] = [
  createManagedEntity(TaskRunEntity, {
    label: "Task runs",
    description: "Scheduled, manual, startup, and catch-up execution history.",
    warning: "Deleting a run also deletes its stored log.",
    canDelete: (row) =>
      row.status === "running" ? "A running task cannot be deleted." : undefined,
    afterDelete: (row) => TaskRunLogEntity.delete({ runId: row.runId }),
  }),
  createManagedEntity(TaskRunLogEntity, {
    label: "Task run logs",
    description: "Captured log lines for completed task runs.",
  }),
  createManagedEntity(TaskScheduleStateEntity, {
    label: "Task schedule state",
    description: "Last evaluated cron occurrence used for catch-up decisions.",
    warning: "Deleting state changes the catch-up baseline for that task.",
  }),
  createManagedEntity(StreamerStatusEntity, {
    label: "Streamer status",
    description: "Current aggregate live session and last offline state per streamer.",
    warning:
      "Deleting live state can cause a fresh went-live transition on the next check.",
  }),
  createManagedEntity(StreamSessionsEntity, {
    label: "Stream sessions",
    description: "Completed live sessions per streamer (start, end, peak, title).",
    warning: "Deleted session history cannot be reconstructed.",
  }),
  createManagedEntity(ViewerMetricsEntity, {
    label: "Viewer metrics",
    description: "Daily viewer peaks and all-time records per streamer.",
    warning:
      "Deleted viewer records cannot be reconstructed outside the retained window.",
  }),
  createManagedEntity(RecommendationEntity, {
    label: "Media recommendations",
    description: "Recommendation attempts, delivery state, outcomes, and feedback.",
    warning: "Deleting rows changes cooldown, exclusion, and taste evidence behavior.",
  }),
  createManagedEntity(PodcastRecommendationEntity, {
    label: "Podcast recommendations",
    description: "Podcast episode picks, queue state, outcomes, and feedback.",
    warning: "Deleting rows changes episode exclusions and show cooldowns.",
  }),
  createManagedEntity(PodcastTasteEvidenceEntity, {
    label: "Podcast taste evidence",
    description: "Listen, outcome, and feedback observations for podcast reflection.",
    warning: "Deleting evidence changes the inputs available to future reflections.",
  }),
  createManagedEntity(PodcastTasteProfileEntity, {
    label: "Podcast taste profiles",
    description: "Generated checkpoints of the current podcast taste model.",
  }),
  createManagedEntity(TasteEvidenceEntity, {
    label: "Taste evidence",
    description: "Versioned observations used to build the media taste profile.",
    warning: "Deleting evidence changes the inputs available to future reflections.",
  }),
  createManagedEntity(TasteProfileEntity, {
    label: "Taste profiles",
    description: "Generated checkpoints of the current media taste model.",
  }),
  createManagedEntity(IdentityAliasEntity, {
    label: "Identity aliases",
    description: "Cached Plex GUID to TMDB identity resolutions.",
    warning: "Deleted aliases will be resolved again when encountered.",
  }),
  createManagedEntity(BriefingHistoryEntity, {
    label: "Briefing history",
    description: "Recent notifications retained for briefing deduplication.",
    warning: "Deleting history can allow a briefing to repeat prior stories.",
  }),
  createManagedEntity(SubmittedDeliveryEntity, {
    label: "Submitted deliveries",
    description: "Parcel tracking numbers already sent to Parcel.app.",
    warning: "This is a deduplication gate. Deleted rows may be submitted again.",
  }),
  createManagedEntity(CreatedCalendarEventEntity, {
    label: "Created calendar events",
    description: "Calendar events created from email, keyed by normalized content.",
    warning: "This is a deduplication gate. Deleted rows may create duplicate events.",
  }),
  createManagedEntity(EmailActivityEntity, {
    label: "Email activity",
    description: "Per-email outcomes from the parcel and calendar pipelines.",
  }),
  createManagedEntity(EmailStateEntity, {
    label: "Email cursor",
    description: "Fastmail JMAP state cursor for incremental email processing.",
    warning: "Deleting the cursor can replay old email through every email handler.",
  }),
];

const ENTITY_BY_SLUG = new Map(MANAGED_ENTITIES.map((entity) => [entity.slug, entity]));

export function listManagedEntities(): ManagedEntitySummary[] {
  return MANAGED_ENTITIES.map((entity) => ({
    slug: entity.slug,
    label: entity.label,
    description: entity.description,
    warning: entity.warning,
    primaryKey: entity.primaryKey,
    count: entity.count(),
    storageBytes: entity.storageBytes(),
  }));
}

export function getManagedDataSummary(
  entities: ManagedEntitySummary[] = listManagedEntities(),
): ManagedDataSummary {
  const db = getDb();
  const pageCount = db.pragma("page_count", { simple: true }) as number;
  const pageSize = db.pragma("page_size", { simple: true }) as number;
  return {
    databaseSizeBytes: pageCount * pageSize,
    entityStorageBytes: entities.reduce(
      (total, entity) => total + entity.storageBytes,
      0,
    ),
  };
}

export function getManagedEntity(
  slug: string,
): { summary: ManagedEntitySummary; rows: DataRow[] } | undefined {
  const entity = ENTITY_BY_SLUG.get(slug);
  if (!entity) return undefined;
  const rows = entity.rows();
  return {
    summary: {
      slug: entity.slug,
      label: entity.label,
      description: entity.description,
      warning: entity.warning,
      primaryKey: entity.primaryKey,
      count: rows.length,
      storageBytes: entity.storageBytes(),
    },
    rows,
  };
}

export function deleteManagedEntityRow(
  slug: string,
  key: DataRowKey,
): DeleteResult | undefined {
  return ENTITY_BY_SLUG.get(slug)?.delete(key);
}
