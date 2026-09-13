import { createHash, randomUUID } from "node:crypto";
import type { NamedLogger } from "@micthiesen/mitools/logging";
import { PushoverError } from "@micthiesen/mitools/pushover";
import { Clock, Effect, Result } from "effect";
import type { ObserverIssue } from "../observer/client.js";
import { type RepairDecision, ObserverRepairError } from "./agent.js";
import {
  acquireIssue,
  listPending,
  releaseIssue,
  saveIssue,
  type ObserverRepairState,
} from "./persistence.js";

export interface RepairDependencies<E, R = never> {
  listOpen(): Effect.Effect<readonly ObserverIssue[], E>;
  getIssue(id: number): Effect.Effect<ObserverIssue, E>;
  assess(issue: ObserverIssue): Effect.Effect<RepairDecision, E, R>;
  prepare(
    issue: ObserverIssue,
    decision: RepairDecision,
  ): Effect.Effect<
    {
      summary: string;
      execute: Effect.Effect<number, E>;
    },
    E
  >;
  comment(id: number, message: string): Effect.Effect<unknown, E>;
  resolve(id: number): Effect.Effect<unknown, E>;
  send(id: number, message: string): Effect.Effect<unknown, E, R>;
}

export function issueRevision(issue: ObserverIssue): string {
  return createHash("sha256")
    .update(
      JSON.stringify({
        id: issue.id,
        status: issue.status,
        type: issue.issueType,
        season: issue.problemSeason,
        episode: issue.problemEpisode,
        media: {
          type: issue.media?.mediaType,
          tmdbId: issue.media?.tmdbId,
          tvdbId: issue.media?.tvdbId,
        },
        comments: issue.comments
          ?.filter((c) => !c.message.startsWith(`[Omni repair ${issue.id}/`))
          .map((c) => [c.id, c.message]),
      }),
    )
    .digest("hex")
    .slice(0, 24);
}

function failureMessage(error: unknown): string {
  // Do not copy arbitrary API bodies or model output into notifications/comments.
  const operation =
    typeof error === "object" && error !== null && "operation" in error
      ? String(error.operation)
      : "repair operation";
  return `Could not complete ${operation}. The issue remains open for manual handling; inspect the task run before retrying.`;
}

export function runObserverRepair<E, R>(
  deps: RepairDependencies<E, R>,
  logger: NamedLogger,
) {
  return Effect.gen(function* () {
    const pending = yield* listPending();
    const open = yield* deps.listOpen();
    const candidates = [
      ...new Set([...pending.map((s) => s.issueId), ...open.map((i) => i.id)]),
    ];
    const summaries: string[] = [];
    const incomplete: number[] = [];
    let handled = 0;
    for (const issueId of candidates) {
      if (handled >= 5) break;
      const issue = yield* deps.getIssue(issueId);
      const previous = pending.find((s) => s.issueId === issueId);
      const owner = yield* Effect.sync(() => randomUUID());
      const state = yield* acquireIssue(
        issueId,
        previous?.revision ?? issueRevision(issue),
        owner,
        yield* Clock.currentTimeMillis,
      );
      if (!state) continue;
      handled += 1;
      const save = () =>
        Clock.currentTimeMillis.pipe(
          Effect.flatMap((now) => saveIssue(state, owner, now)),
        );
      const work = Effect.gen(function* () {
        if (state.phase === "reserved") {
          if (issue.status !== 1) {
            state.phase = "done";
            yield* save();
            return;
          }
          const attempt = yield* Effect.result(
            Effect.gen(function* () {
              const decision = yield* deps.assess(issue);
              yield* logger.info(
                `Observer #${issueId}: ${decision.action}: ${decision.reason}`,
              );
              if (decision.action === "cannot_handle") {
                state.phase = "unhandled";
                state.outcome = "unhandled";
                state.message = decision.reason;
                return;
              }
              const prepared = yield* deps.prepare(issue, decision);
              const fresh = yield* deps.getIssue(issueId);
              if (
                issueRevision(fresh) !== state.revision ||
                fresh.updatedAt !== issue.updatedAt
              )
                return yield* new ObserverRepairError({
                  operation: "recheck changed issue before repair",
                  cause: "Issue changed during assessment",
                });
              state.phase = "executing";
              state.message = prepared.summary;
              yield* save();
              const commandId = yield* prepared.execute.pipe(
                Effect.timeout("10 minutes"),
              );
              state.phase = "repaired";
              state.outcome = "repaired";
              state.message = `${prepared.summary} Automatic search accepted (command ${commandId}); replacement download is not yet verified.`;
            }),
          );
          if (Result.isFailure(attempt)) {
            yield* logger.warn(
              `Observer #${issueId}: ${failureMessage(attempt.failure)}`,
            );
            state.phase = "unhandled";
            state.outcome = "unhandled";
            state.message = failureMessage(attempt.failure);
          }
          yield* save();
        }
        yield* finishIssue(state, deps, save, logger);
        summaries.push(`#${issueId} ${state.outcome ?? "skipped"}`);
      });
      const result = yield* Effect.result(
        work.pipe(
          Effect.ensuring(
            Clock.currentTimeMillis.pipe(
              Effect.flatMap((now) => releaseIssue(issueId, owner, now)),
              Effect.catch((error) =>
                logger.warn(`Observer lease release failed: ${error.operation}`),
              ),
            ),
          ),
        ),
      );
      if (Result.isFailure(result)) {
        yield* logger.warn(
          `Observer #${issueId}: completion pending (${failureMessage(result.failure)})`,
        );
        // Keep the durable phase; retry only unfinished comment/status delivery, never repairs.
        summaries.push(`#${issueId} completion pending`);
        incomplete.push(issueId);
      }
    }
    if (incomplete.length)
      return yield* new ObserverRepairError({
        operation: `complete Observer issues ${incomplete.join(", ")}`,
        cause: "Durable completion remains pending",
      });
    return summaries.length ? summaries.join("; ") : "No unhandled Observer issues";
  });
}

function finishIssue<E, R, SE, SR>(
  state: ObserverRepairState,
  deps: RepairDependencies<E, R>,
  save: () => Effect.Effect<unknown, SE, SR>,
  logger: NamedLogger,
) {
  return Effect.gen(function* () {
    const id = state.issueId;
    if (state.phase === "repaired" || state.phase === "commented") {
      const current = yield* deps.getIssue(id);
      if (current.status === 1 && issueRevision(current) !== state.revision) {
        state.phase = "unhandled";
        state.outcome = "unhandled";
        state.message = `${state.message} The report changed during repair; left open for review.`;
        yield* save();
      }
    }
    if (state.phase === "repaired") {
      const message = `[Omni repair ${id}/${state.revision}] ${state.message}`;
      const current = yield* deps.getIssue(id);
      if (!current.comments?.some((comment) => comment.message === message))
        yield* deps.comment(id, message);
      const verified = yield* deps.getIssue(id);
      if (!verified.comments?.some((comment) => comment.message === message))
        return yield* new ObserverRepairError({
          operation: "verify repair comment",
          cause: "Comment not visible",
        });
      state.phase = "commented";
      yield* save();
    }
    if (state.phase === "commented") {
      const current = yield* deps.getIssue(id);
      if (current.status !== 2 && issueRevision(current) !== state.revision) {
        state.phase = "unhandled";
        state.outcome = "unhandled";
        state.message = `${state.message} The report changed before resolution; left open for review.`;
      } else {
        if (current.status !== 2) yield* deps.resolve(id);
        if ((yield* deps.getIssue(id)).status !== 2)
          return yield* new ObserverRepairError({
            operation: "verify issue resolution",
            cause: "Issue remains open",
          });
        state.phase = "resolved";
      }
      yield* save();
    }
    if (state.notification === "pending") {
      state.notification = "sending";
      yield* save();
      const delivery = yield* Effect.result(
        deps.send(
          id,
          `${state.outcome === "repaired" ? "Repaired" : "Needs attention"}: ${state.message ?? "Repair interrupted; manual review required"}`,
        ),
      );
      if (Result.isFailure(delivery)) {
        const failure = delivery.failure;
        if (
          failure instanceof PushoverError &&
          failure.status !== undefined &&
          failure.status >= 400 &&
          failure.status < 500
        ) {
          state.notification = "pending";
          yield* save();
        }
        return yield* Effect.fail(failure);
      }
      state.notification = "sent";
      yield* save();
    } else if (state.notification === "sending") {
      yield* logger.warn(
        `Observer #${id}: prior notification delivery is uncertain; not sending a duplicate`,
      );
      return yield* new ObserverRepairError({
        operation: "reconcile uncertain Pushover delivery",
        cause: "Delivery remains uncertain; manual reconciliation required",
      });
    }
    state.phase = "done";
    yield* save();
  });
}
