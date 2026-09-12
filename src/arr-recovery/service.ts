import { randomUUID } from "node:crypto";
import { posix } from "node:path";
import type { NamedLogger } from "@micthiesen/mitools/logging";
import { notify, PushoverError } from "@micthiesen/mitools/pushover";
import { Clock, Effect, Result } from "effect";
import config from "../utils/config.js";
import { assessWithLlm } from "./llm.js";
import { downloadHealth } from "./nzbget.js";
import { decide, eligibleQueueItem, observationFingerprint } from "./policy.js";
import {
  acquireState,
  releaseState,
  saveState,
  type Observation,
  type RecoveryAction,
  type RecoveryState,
} from "./persistence.js";
import {
  ArrRecoveryError,
  type ArrClient,
  type Evidence,
  type QueueItem,
  type Target,
} from "./types.js";

export const STUCK_GRACE_MS = 15 * 60_000;
const MAX_OBSERVATION_GAP_MS = 20 * 60_000;
const ASSESSMENT_COOLDOWN_MS = 24 * 60 * 60_000;
const MAX_ACTIONS = 60;
const MAX_LLM_CALLS = 5;
const SEARCH_BACKOFF_MS = 6 * 60 * 60_000;
const SEARCH_WINDOW_MS = 7 * 24 * 60 * 60_000;

export function observe(
  previous: Observation | undefined,
  fingerprint: string,
  now: number,
): Observation {
  if (
    !previous ||
    previous.fingerprint !== fingerprint ||
    now < previous.lastSeenAt ||
    now - previous.lastSeenAt > MAX_OBSERVATION_GAP_MS
  ) {
    return { fingerprint, firstSeenAt: now, lastSeenAt: now, observations: 1 };
  }
  return {
    ...previous,
    lastSeenAt: now,
    observations: previous.observations + (now > previous.lastSeenAt ? 1 : 0),
  };
}
export function isMature(observation: Observation): boolean {
  return (
    observation.observations >= 2 &&
    observation.lastSeenAt - observation.firstSeenAt >= STUCK_GRACE_MS
  );
}
export function groupQueue(queue: QueueItem[]): QueueItem[][] {
  const groups = new Map<string, QueueItem[]>();
  for (const item of queue) {
    if (!item.downloadId) continue;
    const group = groups.get(item.downloadId) ?? [];
    group.push(item);
    groups.set(item.downloadId, group);
  }
  return [...groups.values()];
}
export function sameTarget(a: Target, b: Target): boolean {
  return (
    a.id === b.id &&
    (a.episodeIds.length === 0 || a.episodeIds.some((id) => b.episodeIds.includes(id)))
  );
}
export function canReplace(
  actions: RecoveryAction[],
  target: Target,
  now: number,
  excludingDownloadId?: string,
): boolean {
  const attempts = actions.filter(
    (action) =>
      action.downloadId !== excludingDownloadId &&
      action.decision.action === "remove" &&
      action.decision.replace &&
      sameTarget(action.target, target) &&
      now - action.createdAt < SEARCH_WINDOW_MS,
  );
  return (
    attempts.length < 3 &&
    attempts.every((action) => now - action.createdAt >= SEARCH_BACKOFF_MS)
  );
}
export function sourcePathsConfined(evidence: Evidence): boolean {
  const outputPath = evidence.items[0]?.outputPath;
  if (
    !outputPath ||
    !posix.isAbsolute(outputPath) ||
    posix.normalize(outputPath) === "/"
  )
    return false;
  const library = posix.normalize(evidence.target.path).replace(/\/$/, "");
  const download = posix.normalize(outputPath).replace(/\/$/, "");
  // A misconfigured output path must never turn cleanup into a library deletion.
  if (
    !posix.isAbsolute(library) ||
    library === download ||
    library.startsWith(`${download}/`) ||
    download.startsWith(`${library}/`)
  )
    return false;
  return evidence.files.every(
    (file) =>
      file.size > 0 &&
      posix.isAbsolute(file.path) &&
      file.path.startsWith(`${outputPath.replace(/\/$/, "")}/`) &&
      !posix.relative(outputPath, file.path).startsWith(".."),
  );
}
export const gatherEvidence = Effect.fn("ArrRecovery.gatherEvidence")(function* (
  client: ArrClient,
  items: QueueItem[],
) {
  const target = yield* client.target(items);
  const files = yield* client.preview(items[0].downloadId);
  const grabs = yield* client.history(items[0].downloadId);
  const health =
    files.length === 0 && config.NZBGET_URL && items[0].outputPath
      ? yield* downloadHealth(
          config.NZBGET_URL,
          client.kind,
          items[0].downloadId,
          items[0].outputPath,
        )
      : undefined;
  return {
    kind: client.kind,
    items,
    target,
    files,
    grabs,
    downloadHealth: health,
  } satisfies Evidence;
});

function currentEvidenceKey(evidence: Evidence): string {
  return JSON.stringify({
    target: evidence.target,
    files: evidence.files,
    grabs: evidence.grabs,
    downloadHealth: evidence.downloadHealth,
  });
}

export function notificationMessage(actions: RecoveryAction[]): string {
  const groups = new Map<string, number>();
  for (const action of actions) {
    const verb =
      action.phase !== "done"
        ? "Needs inspection"
        : action.decision.action === "import"
          ? "Imported"
          : action.decision.action === "remove" && action.decision.replace
            ? action.commandId !== undefined
              ? "Removed; replacement search requested"
              : "Removed; replacement already queued"
            : "Removed redundant download";
    const key = `${verb}: ${action.target.title.slice(0, 200)}${action.decision.source === "llm" ? " (Luna)" : ""}`;
    groups.set(key, (groups.get(key) ?? 0) + 1);
  }
  return [...groups]
    .map(([key, count]) => `${key}${count > 1 ? ` (${count} downloads)` : ""}`)
    .join("\n");
}

export interface RecoveryOptions {
  assess?: typeof assessWithLlm;
  send?: (message: string) => Effect.Effect<void, ArrRecoveryError>;
}

export const runRecovery = Effect.fn("ArrRecovery.run")(function* (
  clients: ArrClient[],
  logger: NamedLogger,
  options: RecoveryOptions = {},
) {
  const summaries: string[] = [];
  const errors: string[] = [];
  for (const client of clients) {
    const result = yield* Effect.result(runClient(client, logger, options));
    if (Result.isFailure(result)) {
      errors.push(`${client.kind}: ${result.failure.message}`);
      yield* logger.warn(
        `Arr recovery failed for ${client.kind}`,
        result.failure.message,
      );
    } else summaries.push(result.success);
  }
  if (errors.length > 0)
    return yield* new ArrRecoveryError({
      operation: "recover Arr queues",
      cause: [...summaries, ...errors].join("; "),
    });
  return summaries.join("; ");
});

const runClient = Effect.fn("ArrRecovery.runClient")(function* (
  client: ArrClient,
  logger: NamedLogger,
  options: RecoveryOptions,
) {
  const owner = yield* Effect.sync(() => randomUUID());
  const state = yield* acquireState(client.kind, owner, yield* Clock.currentTimeMillis);
  if (!state) return `${client.kind}: another recovery run holds the lease`;
  const save = () =>
    Clock.currentTimeMillis.pipe(Effect.flatMap((now) => saveState(state, owner, now)));
  const work = Effect.gen(function* () {
    const queue = yield* client.queue();
    const groups = groupQueue(queue);
    const now = yield* Clock.currentTimeMillis;
    const eligible = groups.filter((items) => items.every(eligibleQueueItem));
    const eligibleIds = new Set(eligible.map((items) => items[0].downloadId));
    for (const id of Object.keys(state.observations))
      if (!eligibleIds.has(id)) delete state.observations[id];
    // Retain all incomplete attempts and notification reservations, plus a month of completed history.
    state.actions = state.actions.filter(
      (a) =>
        a.phase !== "done" ||
        a.notification !== "sent" ||
        now - a.createdAt < 30 * 24 * 60 * 60_000,
    );
    for (const items of eligible) {
      const id = items[0].downloadId;
      state.observations[id] = observe(
        state.observations[id],
        observationFingerprint(items),
        now,
      );
    }
    yield* save();
    const failures: string[] = [];
    let acted = 0;
    let llmCalls = 0;
    for (const action of state.actions.filter((a) => a.phase !== "done")) {
      const result = yield* Effect.result(reconcile(client, state, action, save));
      if (Result.isFailure(result)) {
        action.error = result.failure.message;
        failures.push(action.error);
        yield* save();
      }
    }
    for (const items of eligible) {
      if (acted >= MAX_ACTIONS) break;
      const id = items[0].downloadId;
      const observation = state.observations[id];
      if (!isMature(observation) || state.actions.some((a) => a.downloadId === id))
        continue;
      if (
        observation.lastAssessedAt !== undefined &&
        now - observation.lastAssessedAt < ASSESSMENT_COOLDOWN_MS
      )
        continue;
      const result = yield* Effect.result(
        Effect.gen(function* () {
          const evidence = yield* gatherEvidence(client, items);
          let decision = decide(evidence);
          if (decision.action === "defer") {
            if (llmCalls >= MAX_LLM_CALLS) return;
            llmCalls++;
            decision = yield* (options.assess ?? assessWithLlm)(evidence);
          }
          if (
            decision.action === "remove" &&
            decision.replace &&
            !canReplace(state.actions, evidence.target, now)
          ) {
            decision = {
              action: "defer",
              reason: "Replacement search budget/backoff reached",
              source: "rules",
            };
          }
          observation.lastAssessedAt = now;
          observation.reason = decision.reason;
          yield* save();
          yield* logger.info(
            `${client.kind}: ${decision.action} ${items[0].title}: ${decision.reason}`,
          );
          if (decision.action === "defer") return;
          if (!sourcePathsConfined(evidence))
            return yield* new ArrRecoveryError({
              operation: "validate download paths",
              cause: "Missing, empty, or unconfined source path",
            });
          // Normal Arr work or a human may have resolved this during assessment.
          const freshItems = (yield* client.queue()).filter(
            (item) => item.downloadId === id,
          );
          if (
            freshItems.length !== items.length ||
            !freshItems.every(eligibleQueueItem) ||
            observationFingerprint(freshItems) !== observation.fingerprint
          ) {
            delete state.observations[id];
            yield* save();
            return;
          }
          const fresh = yield* gatherEvidence(client, freshItems);
          if (currentEvidenceKey(fresh) !== currentEvidenceKey(evidence)) {
            delete state.observations[id];
            yield* save();
            return;
          }
          const action: RecoveryAction = {
            downloadId: id,
            title: items[0].title,
            target: evidence.target,
            files: evidence.files,
            outputPath: items[0].outputPath!,
            decision,
            phase: "reserved",
            createdAt: now,
            updatedAt: now,
            notification: "pending",
          };
          state.actions.push(action);
          yield* save();
          acted++;
          const mutation = yield* Effect.result(
            Effect.gen(function* () {
              if (decision.action === "import") {
                action.commandId = yield* client.importFiles(id, evidence.files);
                action.phase = "submitted";
                yield* save();
                yield* Effect.sleep("2 seconds");
              } else {
                yield* client.remove(freshItems[0].id, decision.replace);
                action.phase = "removed";
                yield* save();
              }
              yield* reconcile(client, state, action, save);
            }),
          );
          if (Result.isFailure(mutation)) {
            action.error = mutation.failure.message;
            if (action.phase === "reserved") action.phase = "uncertain";
            failures.push(action.error);
            yield* save();
          }
        }),
      );
      if (Result.isFailure(result)) {
        failures.push(result.failure.message);
        yield* logger.warn(
          `${client.kind}: recovery deferred for ${items[0].title}`,
          result.failure.message,
        );
      }
    }
    yield* deliverNotifications(state, save, options);
    const summary = `${client.kind}: ${acted} action(s), ${state.actions.filter((a) => a.phase !== "done").length} awaiting verification, ${eligible.length} stuck candidate(s), ${llmCalls} Luna assessment(s)`;
    yield* logger.info(summary);
    if (failures.length)
      return yield* new ArrRecoveryError({
        operation: summary,
        cause: failures.slice(0, 5).join("; "),
      });
    return summary;
  });
  return yield* work.pipe(
    Effect.timeout("20 minutes"),
    Effect.mapError(
      (cause) => new ArrRecoveryError({ operation: "recover queue", cause }),
    ),
    Effect.ensuring(
      releaseState(client.kind, owner).pipe(
        Effect.catch((error) =>
          logger.warn("Could not release Arr recovery lease", error),
        ),
      ),
    ),
  );
});

const reconcile = Effect.fn("ArrRecovery.reconcile")(function* (
  client: ArrClient,
  state: RecoveryState,
  action: RecoveryAction,
  save: () => ReturnType<typeof saveState>,
) {
  if (action.decision.action === "defer") return;
  if (action.decision.action === "import") {
    if (yield* client.verifyImported(action.target, action.files)) {
      action.phase = "done";
      action.error = undefined;
      action.updatedAt = yield* Clock.currentTimeMillis;
      yield* save();
      return;
    }
    if (action.commandId !== undefined) {
      const command = yield* client.command(action.commandId);
      if (["queued", "started"].includes(command.status)) return;
      action.error = `Import command ${command.status}; expected files not verified`;
    } else action.error = "Import submission outcome unknown; not submitting it twice";
    action.phase = "uncertain";
    yield* save();
    return;
  }
  const queue = yield* client.queue();
  if (
    queue.some((item) => item.downloadId === action.downloadId) ||
    !(yield* client.verifyRemoved(action.outputPath))
  ) {
    action.error = "Download removal/file deletion not yet verified";
    yield* save();
    return;
  }
  if (!action.decision.replace) {
    action.phase = "done";
    action.error = undefined;
    action.updatedAt = yield* Clock.currentTimeMillis;
    yield* save();
    return;
  }
  if (
    action.phase === "searching" ||
    (action.phase === "uncertain" && action.commandId !== undefined)
  ) {
    // The command id proves acceptance. An unknown submission is held, never blindly repeated.
    if (action.commandId === undefined) return;
    const command = yield* client.command(action.commandId);
    if (command.status === "failed" || command.status === "aborted") {
      action.phase = "uncertain";
      action.error = `Replacement search ${command.status}`;
    } else {
      action.phase = "done";
      action.error = undefined;
    }
    yield* save();
    return;
  }
  // Refresh monitoring and file presence after removal. Search only still-missing monitored targets.
  const freshTarget = yield* client.target(
    action.target.episodeIds.length > 0
      ? action.target.episodeIds.map(
          (episodeId) => ({ seriesId: action.target.id, episodeId }) as QueueItem,
        )
      : [{ movieId: action.target.id } as QueueItem],
  );
  const missingEpisodes = freshTarget.episodes.filter(
    (episode) => !episode.hasFile && episode.monitored,
  );
  if (
    !freshTarget.monitored ||
    freshTarget.hasFile ||
    (client.kind === "sonarr" && missingEpisodes.length === 0)
  ) {
    action.decision = {
      ...action.decision,
      replace: false,
      reason: "Removed; target already satisfied or unmonitored",
    };
    action.phase = "done";
    yield* save();
    return;
  }
  const searchTarget =
    client.kind === "sonarr"
      ? {
          ...freshTarget,
          episodes: missingEpisodes,
          episodeIds: missingEpisodes.map((episode) => episode.id),
        }
      : freshTarget;
  if (
    queue.some(
      (item) =>
        item.downloadId !== action.downloadId &&
        (client.kind === "radarr"
          ? item.movieId === freshTarget.id
          : item.seriesId === freshTarget.id &&
            searchTarget.episodeIds.includes(item.episodeId ?? -1)),
    )
  ) {
    action.phase = "done";
    action.error = undefined;
    yield* save();
    return;
  }
  const now = yield* Clock.currentTimeMillis;
  if (!canReplace(state.actions, searchTarget, now, action.downloadId)) return;
  action.phase = "searching";
  action.commandId = undefined;
  yield* save();
  action.commandId = yield* client.search(searchTarget);
  yield* save();
  action.phase = "done";
  action.error = undefined;
  action.updatedAt = now;
  yield* save();
});

function deliverNotifications(
  state: RecoveryState,
  save: () => ReturnType<typeof saveState>,
  options: RecoveryOptions,
) {
  return Effect.gen(function* () {
    const pending = state.actions.filter(
      (a) => ["done", "uncertain"].includes(a.phase) && a.notification === "pending",
    );
    // Each request stays within Pushover's 1,024-character message limit.
    for (const batch of notificationBatches(pending)) {
      const message = notificationMessage(batch);
      for (const action of batch) action.notification = "sending";
      yield* save();
      const delivery = yield* Effect.result(
        options.send
          ? options.send(message)
          : notify({
              title: `Omni ${state.kind} recovery`,
              message,
              token: config.PUSHOVER_RECS_TOKEN,
              url: "http://omni.boris/",
              url_title: "View task runs",
            }).pipe(
              Effect.mapError(
                (cause) =>
                  new ArrRecoveryError({
                    operation: "send recovery notification",
                    cause,
                  }),
              ),
            ),
      );
      if (Result.isFailure(delivery)) {
        const cause = delivery.failure.cause;
        // A 4xx rejection confirms the provider did not accept the batch. Network
        // loss/5xx/crashes remain reserved because acceptance cannot be disproved.
        if (
          cause instanceof PushoverError &&
          cause.status !== undefined &&
          cause.status >= 400 &&
          cause.status < 500
        ) {
          for (const action of batch) action.notification = "pending";
        }
        yield* save();
        return yield* delivery.failure;
      }
      for (const action of batch) action.notification = "sent";
      yield* save();
    }
  });
}

export function notificationBatches(actions: RecoveryAction[]): RecoveryAction[][] {
  const batches: RecoveryAction[][] = [];
  let batch: RecoveryAction[] = [];
  for (const action of actions) {
    if (batch.length > 0 && notificationMessage([...batch, action]).length > 1000) {
      batches.push(batch);
      batch = [];
    }
    batch.push(action);
  }
  if (batch.length > 0) batches.push(batch);
  return batches;
}
