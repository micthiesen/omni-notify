import type { Entity } from "@micthiesen/mitools/entities";
import { BriefingHistoryEntity } from "./briefing-agent/persistence.js";
import { CreatedCalendarEventEntity } from "./calendar-events/persistence.js";
import { EmailStateEntity } from "./jmap/persistence.js";
import { ViewerMetricsEntity } from "./live-check/metrics/persistence.js";
import { StreamerStatusEntity } from "./live-check/persistence.js";
import { SubmittedDeliveryEntity } from "./parcel-tracker/persistence.js";
import { PodcastRecommendationEntity } from "./podcast-recs/persistence.js";
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
}

interface EntityOptions<Data> {
  label: string;
  description: string;
  warning?: string;
  canDelete?: (row: Data) => string | undefined;
  afterDelete?: (row: Data) => void;
}

interface ManagedEntity extends Omit<ManagedEntitySummary, "count"> {
  count(): number;
  rows(): DataRow[];
  delete(key: DataRowKey): DeleteResult;
}

export type DeleteResult =
  | { status: "deleted"; row: DataRow }
  | { status: "invalid-key" }
  | { status: "not-found" }
  | { status: "blocked"; reason: string };

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
    rows: () => entity.getAll() as DataRow[],
    delete: (key) => {
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
  }));
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
