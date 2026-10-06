import { createHash } from "node:crypto";
import { Entity } from "@micthiesen/mitools/entities";
import { Clock, Duration, Effect, Option, Schema } from "effect";
import type { DeviceLinkService } from "../../device-link/service.js";
import { CLAUDE_TURN_FINISHED } from "./catalog.js";
import type { McpEventService } from "./service.js";

/** The last turn state Omni saw for one session on the Claude Code host. */
export interface ClaudeSessionWatch {
  sessionId: string;
  id: string | null;
  project: string | null;
  revision: number;
  settled: boolean;
  /** Omni began a turn at `revision`; only a later revision can finish it. */
  started?: boolean;
  /** Consecutive failed lookups after the session left the running list. */
  misses?: number;
  seenAt: number;
}

export const ClaudeSessionWatchEntity = new Entity<ClaudeSessionWatch, ["sessionId"]>(
  "mcp-claude-session-watch",
  ["sessionId"],
);

const watchSchema = Schema.Struct({
  sessionId: Schema.String,
  id: Schema.NullOr(Schema.String),
  project: Schema.NullOr(Schema.String),
  revision: Schema.Number,
  settled: Schema.Boolean,
  started: Schema.optional(Schema.Boolean),
  misses: Schema.optional(Schema.Number),
  seenAt: Schema.Number,
});

/** One session summary as claude-for-dot reports it. */
const summarySchema = Schema.Struct({
  session_id: Schema.String,
  id: Schema.optional(Schema.NullOr(Schema.String)),
  project: Schema.optional(Schema.NullOr(Schema.String)),
  status: Schema.String,
  state: Schema.optional(Schema.NullOr(Schema.String)),
  revision: Schema.Number,
});
type Summary = typeof summarySchema.Type;

const listSchema = Schema.Struct({ sessions: Schema.Array(Schema.Unknown) });

const LIST_LIMIT = 100;
/** Sessions that left the running list are looked up individually, a few per pass. */
const MAX_STATUS_LOOKUPS = 5;
/** A session that cannot be read this many polls in a row is forgotten. */
const MAX_MISSES = 10;
const WATCH_RETENTION_MS = 7 * 24 * 60 * 60_000;
const COMMAND_TIMEOUT = Duration.seconds(60);
/** A turn last seen this long ago is history, not news, when polling resumes. */
const STALE_TURN_MS = 24 * 60 * 60_000;

/** Same rule as claude-for-dot's wait: idle with no turn in progress, or stopped. */
export function isSettled(summary: Pick<Summary, "status" | "state">): boolean {
  return (
    summary.status === "stopped" ||
    (summary.status !== "busy" && summary.state !== "working")
  );
}

/**
 * A finished turn is a settled session whose transcript moved past the turn
 * Omni last saw in progress, or that settled since Omni last saw it working.
 * A session seen for the first time only sets the baseline. A turn Omni just
 * started can report idle before it begins, so it needs a later revision.
 */
export function finishedTurn(
  previous: Pick<ClaudeSessionWatch, "revision" | "settled" | "started"> | undefined,
  summary: Pick<Summary, "status" | "state" | "revision">,
): boolean {
  if (!previous || !isSettled(summary)) return false;
  if (previous.started) return summary.revision > previous.revision;
  return !previous.settled || summary.revision > previous.revision;
}

const decodeSummary = (raw: unknown) =>
  Schema.decodeUnknownEffect(summarySchema)(raw).pipe(Effect.option);

/**
 * Publishes `claude.session.turn_finished` by diffing the host's session list.
 * It polls only while a subscription for the event is active and the host is
 * online, so the host's tools stay the only way to observe sessions otherwise.
 */
export class ClaudeSessionWatcher {
  constructor(
    private readonly events: McpEventService,
    private readonly link: DeviceLinkService,
  ) {}

  /**
   * Records a turn Omni just started, so a turn that ends before the next poll
   * still produces an event.
   */
  noteTurnStarted(input: {
    sessionId: string;
    id: string | null;
    project: string | null;
    revision: number;
  }) {
    return Clock.currentTimeMillis.pipe(
      Effect.flatMap((now) =>
        ClaudeSessionWatchEntity.upsert({
          ...input,
          settled: false,
          started: true,
          seenAt: now,
        }),
      ),
    );
  }

  poll() {
    return Effect.gen({ self: this }, function* () {
      if (!(yield* this.events.hasActiveSubscription(CLAUDE_TURN_FINISHED))) return;
      const link = yield* this.link.status();
      if (!link.online || link.disabled) return;
      const listed = yield* this.link
        .execute("list", { all: false, limit: LIST_LIMIT }, COMMAND_TIMEOUT)
        .pipe(Effect.flatMap(Schema.decodeUnknownEffect(listSchema)));
      const watches = new Map(
        (yield* Effect.forEach(yield* ClaudeSessionWatchEntity.getAll(), (row) =>
          Schema.decodeUnknownEffect(watchSchema)(row),
        )).map((watch) => [watch.sessionId, watch]),
      );
      const seen = new Set<string>();
      for (const raw of listed.sessions) {
        const summary = yield* decodeSummary(raw);
        if (Option.isNone(summary)) continue;
        seen.add(summary.value.session_id);
        yield* this.observe(watches.get(summary.value.session_id), summary.value);
      }
      // A stopped session leaves the running list; read the ones mid-turn,
      // least-missed first so one unreadable session cannot starve the rest.
      const missing = [...watches.values()]
        .filter((watch) => !watch.settled && !seen.has(watch.sessionId))
        .sort((a, b) => (a.misses ?? 0) - (b.misses ?? 0))
        .slice(0, MAX_STATUS_LOOKUPS);
      for (const watch of missing) {
        const status = yield* this.link
          .execute("status", { session: watch.sessionId }, COMMAND_TIMEOUT)
          .pipe(Effect.result);
        const summary =
          status._tag === "Success"
            ? yield* decodeSummary(status.success)
            : Option.none();
        if (Option.isSome(summary)) {
          yield* this.observe(watch, summary.value);
          continue;
        }
        const misses = (watch.misses ?? 0) + 1;
        const gone = status._tag === "Failure" && status.failure.code === "not_found";
        yield* gone || misses >= MAX_MISSES
          ? ClaudeSessionWatchEntity.delete({ sessionId: watch.sessionId })
          : ClaudeSessionWatchEntity.upsert({ ...watch, misses });
      }
      const now = yield* Clock.currentTimeMillis;
      for (const watch of watches.values()) {
        if (!seen.has(watch.sessionId) && watch.seenAt < now - WATCH_RETENTION_MS) {
          yield* ClaudeSessionWatchEntity.delete({ sessionId: watch.sessionId });
        }
      }
    });
  }

  private observe(previous: ClaudeSessionWatch | undefined, summary: Summary) {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      const project = summary.project ?? previous?.project ?? null;
      const id = summary.id ?? previous?.id ?? null;
      const recent = previous !== undefined && previous.seenAt > now - STALE_TURN_MS;
      if (recent && finishedTurn(previous, summary)) {
        const key = `${CLAUDE_TURN_FINISHED}:${summary.session_id}:${summary.revision}`;
        const receiptKey = createHash("sha256").update(key).digest("hex");
        yield* this.events.publish({
          name: CLAUDE_TURN_FINISHED,
          receiptKey,
          eventKey: key,
          timestamp: new Date(now).toISOString(),
          data: {
            sessionId: summary.session_id,
            id,
            project,
            status: summary.status,
            revision: summary.revision,
          },
        });
      }
      yield* ClaudeSessionWatchEntity.upsert({
        sessionId: summary.session_id,
        id,
        project,
        revision: summary.revision,
        settled: isSettled(summary),
        seenAt: now,
      });
    });
  }
}
