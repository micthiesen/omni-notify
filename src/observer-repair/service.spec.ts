import { Docstore } from "@micthiesen/mitools/docstore";
import { Logger, type NamedLogger } from "@micthiesen/mitools/logging";
import { expect, layer } from "@effect/vitest";
import { Effect, Layer } from "effect";
import { TestClock } from "effect/testing";
import type { ObserverIssue } from "../observer/client.js";
import type { RepairDecision } from "./agent.js";
import {
  acquireIssue,
  saveIssue,
  OBSERVER_REPAIR_LEASE_MS,
  ObserverRepairStateEntity,
} from "./persistence.js";
import {
  issueRevision,
  runObserverRepair,
  type RepairDependencies,
} from "./service.js";

const NOW = 1_700_000_000_000;
const logger = {
  info: () => Effect.void,
  warn: () => Effect.void,
} as unknown as NamedLogger;

function issue(id = 1): ObserverIssue {
  return {
    id,
    issueType: 1,
    status: 1,
    problemSeason: 1,
    problemEpisode: 1,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    media: {
      id: 10,
      tmdbId: 20,
      tvdbId: 30,
      status: 5,
      mediaType: "tv",
      title: "Example",
      externalServiceId: 40,
      externalServiceId4k: null,
      externalServiceSlug: "example",
      externalServiceSlug4k: null,
      serviceId: 50,
      serviceId4k: null,
      serviceUrl: "http://arr",
    },
    createdBy: null,
    modifiedBy: null,
    comments: [],
  };
}

function depsFor(
  current: ObserverIssue,
  decision: RepairDecision,
  overrides: Partial<RepairDependencies<Error, never>> = {},
) {
  let assessed = 0;
  let executed = 0;
  let comments = 0;
  let resolved = 0;
  let sent = 0;
  let currentStatus = current.status;
  let currentComments = current.comments ?? [];
  const deps: RepairDependencies<Error> = {
    listOpen: () =>
      Effect.succeed(
        currentStatus === 1
          ? [{ ...current, status: currentStatus, comments: currentComments }]
          : [],
      ),
    getIssue: () =>
      Effect.succeed({ ...current, status: currentStatus, comments: currentComments }),
    assess: () =>
      Effect.sync(() => {
        assessed += 1;
        return decision;
      }),
    prepare: () =>
      Effect.succeed({
        summary: "Replacement requested",
        execute: Effect.sync(() => {
          executed += 1;
          return 17;
        }),
      }),
    comment: (_id, message) =>
      Effect.sync(() => {
        comments += 1;
        currentComments = [
          ...currentComments,
          {
            id: comments,
            message,
            user: null,
            createdAt: null,
            updatedAt: null,
          },
        ];
      }),
    resolve: () =>
      Effect.sync(() => {
        resolved += 1;
        currentStatus = 2;
      }),
    send: () =>
      Effect.sync(() => {
        sent += 1;
      }),
    ...overrides,
  };
  return {
    deps,
    counts: () => ({ assessed, executed, comments, resolved, sent }),
    setStatus: (status: number) => {
      currentStatus = status;
    },
    addComment: (message: string) => {
      currentComments = [
        ...currentComments,
        {
          id: currentComments.length + 1,
          message,
          user: null,
          createdAt: null,
          updatedAt: null,
        },
      ];
    },
  };
}

layer(Layer.merge(Docstore.layerMemory, Logger.layer()))(
  "Observer repair service",
  (it) => {
    it.effect("repairs, comments, resolves, and notifies once", () =>
      Effect.gen(function* () {
        yield* ObserverRepairStateEntity.deleteAll();
        const current = issue();
        const setup = depsFor(current, {
          action: "replace",
          season: 1,
          episodes: [1],
          scopeComment: null,
          reason: "Bad file",
        });
        expect(yield* runObserverRepair(setup.deps, logger)).toContain("repaired");
        expect(setup.counts()).toEqual({
          assessed: 1,
          executed: 1,
          comments: 1,
          resolved: 1,
          sent: 1,
        });
        expect(yield* runObserverRepair(setup.deps, logger)).toBe(
          "No unhandled Observer issues",
        );
        expect(setup.counts().sent).toBe(1);
      }),
    );

    it.effect("cannot_handle leaves the issue open and notifies once", () =>
      Effect.gen(function* () {
        yield* ObserverRepairStateEntity.deleteAll();
        const current = issue(2);
        const setup = depsFor(current, {
          action: "cannot_handle",
          season: null,
          episodes: [],
          scopeComment: null,
          reason: "Unsupported player",
        });
        yield* runObserverRepair(setup.deps, logger);
        expect(current.status).toBe(1);
        expect(setup.counts()).toEqual({
          assessed: 1,
          executed: 0,
          comments: 0,
          resolved: 0,
          sent: 1,
        });
        yield* runObserverRepair(setup.deps, logger);
        expect(setup.counts().sent).toBe(1);
      }),
    );

    it.effect("does not resolve after repair failure and does not repeat it", () =>
      Effect.gen(function* () {
        yield* ObserverRepairStateEntity.deleteAll();
        const current = issue(3);
        const setup = depsFor(
          current,
          {
            action: "replace",
            season: 1,
            episodes: [1],
            scopeComment: null,
            reason: "Bad file",
          },
          {
            prepare: () => Effect.fail(new Error("arr unavailable")),
          },
        );
        yield* runObserverRepair(setup.deps, logger);
        expect(setup.counts()).toMatchObject({ executed: 0, resolved: 0, sent: 1 });
        yield* runObserverRepair(setup.deps, logger);
        expect(setup.counts().sent).toBe(1);
      }),
    );

    it.effect(
      "resumes an interrupted execution as unhandled without assessing or repairing",
      () =>
        Effect.gen(function* () {
          yield* ObserverRepairStateEntity.deleteAll();
          const current = issue(4);
          const owner = "crashed-worker";
          const seedNow = NOW - OBSERVER_REPAIR_LEASE_MS - 1;
          const reserved = yield* acquireIssue(
            current.id,
            issueRevision(current),
            owner,
            seedNow,
          );
          yield* saveIssue({ ...reserved!, phase: "executing" }, owner, seedNow + 1);
          const setup = depsFor(current, {
            action: "replace",
            season: 1,
            episodes: [1],
            scopeComment: null,
            reason: "Bad file",
          });
          yield* TestClock.setTime(NOW);
          yield* runObserverRepair(setup.deps, logger);
          expect(setup.counts()).toMatchObject({
            assessed: 0,
            executed: 0,
            resolved: 0,
            sent: 1,
          });
        }),
    );

    it.effect("retries comment completion without repeating repair", () =>
      Effect.gen(function* () {
        yield* ObserverRepairStateEntity.deleteAll();
        const current = issue(5);
        const setup = depsFor(current, {
          action: "replace",
          season: 1,
          episodes: [1],
          scopeComment: null,
          reason: "Bad file",
        });
        const comment = setup.deps.comment;
        setup.deps.comment = () => Effect.fail(new Error("Observer unavailable"));
        const first = yield* Effect.result(runObserverRepair(setup.deps, logger));
        expect(first._tag).toBe("Failure");
        expect(setup.counts()).toMatchObject({ executed: 1, resolved: 0 });
        setup.deps.comment = comment;
        const second = yield* Effect.result(runObserverRepair(setup.deps, logger));
        expect(second._tag).toBe("Success");
        expect(setup.counts().executed).toBe(1);
      }),
    );

    it.effect("blocks repair when the issue changes during assessment", () =>
      Effect.gen(function* () {
        yield* ObserverRepairStateEntity.deleteAll();
        const current = issue(6);
        const setup = depsFor(current, {
          action: "replace",
          season: 1,
          episodes: [1],
          scopeComment: null,
          reason: "Bad file",
        });
        const assess = setup.deps.assess;
        setup.deps.assess = (report) =>
          Effect.sync(() => setup.addComment("Actually, the scope changed")).pipe(
            Effect.flatMap(() => assess(report)),
          );
        yield* runObserverRepair(setup.deps, logger);
        expect(setup.counts()).toMatchObject({ executed: 0, resolved: 0, sent: 1 });
      }),
    );

    it.effect(
      "reassesses a completed issue when it is reopened with a human comment",
      () =>
        Effect.gen(function* () {
          yield* ObserverRepairStateEntity.deleteAll();
          const current = issue(7);
          const setup = depsFor(current, {
            action: "replace",
            season: 1,
            episodes: [1],
            scopeComment: null,
            reason: "Bad file",
          });
          yield* runObserverRepair(setup.deps, logger);
          setup.setStatus(1);
          setup.addComment("Please retry this report");
          yield* runObserverRepair(setup.deps, logger);
          expect(setup.counts()).toMatchObject({ assessed: 2, executed: 2, sent: 2 });
        }),
    );

    it.effect(
      "does not repeat repair when a changed comment interrupts completion",
      () =>
        Effect.gen(function* () {
          yield* ObserverRepairStateEntity.deleteAll();
          const current = issue(8);
          let failResolve = true;
          const setup = depsFor(
            current,
            {
              action: "replace",
              season: 1,
              episodes: [1],
              scopeComment: null,
              reason: "Bad file",
            },
            {
              resolve: () => {
                if (failResolve) {
                  failResolve = false;
                  return Effect.fail(new Error("Observer unavailable"));
                }
                return Effect.void;
              },
            },
          );
          const first = yield* Effect.result(runObserverRepair(setup.deps, logger));
          expect(first._tag).toBe("Failure");
          setup.addComment("A new detail from the user");
          yield* runObserverRepair(setup.deps, logger);
          expect(setup.counts()).toMatchObject({
            assessed: 1,
            executed: 1,
            resolved: 0,
          });
        }),
    );
  },
);
