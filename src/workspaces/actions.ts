import type { NamedLogger } from "@micthiesen/mitools/logging";
import { Effect, Schema } from "effect";
import {
  createCalendarEventEffect,
  discoverCaldavSessionEffect,
} from "../calendar-events/caldav/index.js";
import { WorkspaceActionError, WorkspaceValidationError } from "./errors.js";
import {
  getWorkspaceAction,
  setWorkspaceActionResult,
  upsertWorkspaceEmailScope,
} from "./persistence.js";
import { workspaceRepositoryEffect } from "./repository.js";
import type {
  WorkspaceCalendarEventPayload,
  WorkspaceEmailScopePayload,
} from "./types.js";

const EmailScopePayloadSchema = Schema.Struct({
  senders: Schema.Array(Schema.String),
  domains: Schema.Array(Schema.String),
  subjectKeywords: Schema.Array(Schema.String),
  bodyKeywords: Schema.Array(Schema.String),
});
const CalendarEventPayloadSchema = Schema.Struct({
  title: Schema.String,
  startDate: Schema.String,
  endDate: Schema.optional(Schema.String),
  startTime: Schema.optional(Schema.String),
  endTime: Schema.optional(Schema.String),
  location: Schema.optional(Schema.String),
  description: Schema.optional(Schema.String),
  timeZone: Schema.optional(Schema.String),
  allDay: Schema.Boolean,
  reminderMinutes: Schema.optional(Schema.Number),
});

const resolvingActions = new Set<string>();

function decodePayload<A, I>(
  actionId: string,
  payload: string,
  schema: Schema.Codec<A, I>,
) {
  return Effect.try({
    try: () => JSON.parse(payload) as unknown,
    catch: (cause) =>
      new WorkspaceValidationError({
        message: `Workspace action ${actionId} has invalid JSON`,
        cause,
      }),
  }).pipe(
    Effect.flatMap((value) =>
      Schema.decodeUnknownEffect(schema)(value).pipe(
        Effect.mapError(
          (cause) =>
            new WorkspaceValidationError({
              message: `Workspace action ${actionId} has an invalid payload`,
              cause,
            }),
        ),
      ),
    ),
  );
}

function getApprovableActionEffect(actionId: string) {
  return workspaceRepositoryEffect("read workspace action", () =>
    getWorkspaceAction(actionId),
  ).pipe(
    Effect.flatMap((action) => {
      if (!action)
        return Effect.fail(
          new WorkspaceActionError({ actionId, message: "Workspace action not found" }),
        );
      if (action.status !== "pending" && action.status !== "failed") {
        return Effect.fail(
          new WorkspaceActionError({
            actionId,
            message: `Workspace action is already ${action.status}`,
          }),
        );
      }
      return Effect.succeed(action);
    }),
  );
}

function finishEffect(
  actionId: string,
  status: "approved" | "rejected",
  result: string,
) {
  return workspaceRepositoryEffect("resolve workspace action", () =>
    setWorkspaceActionResult(actionId, status, result),
  ).pipe(
    Effect.flatMap((action) =>
      action
        ? Effect.succeed(action)
        : Effect.fail(
            new WorkspaceActionError({
              actionId,
              message: "Workspace action disappeared while resolving it",
            }),
          ),
    ),
  );
}

export function approveWorkspaceActionEffect(actionId: string, logger: NamedLogger) {
  const acquire = Effect.gen(function* () {
    if (resolvingActions.has(actionId))
      return yield* new WorkspaceActionError({
        actionId,
        message: "Workspace action is already being resolved",
      });
    const action = yield* getApprovableActionEffect(actionId);
    resolvingActions.add(actionId);
    return action;
  });
  return Effect.acquireUseRelease(
    acquire,
    (action) =>
      Effect.gen(function* () {
        if (action.type === "email_scope") {
          const scope = yield* decodePayload(
            actionId,
            action.payload,
            EmailScopePayloadSchema,
          );
          const matchers = [
            ...scope.senders,
            ...scope.domains,
            ...scope.subjectKeywords,
            ...scope.bodyKeywords,
          ];
          if (
            [
              scope.senders,
              scope.domains,
              scope.subjectKeywords,
              scope.bodyKeywords,
            ].some((values) => values.length > 20) ||
            matchers.length === 0 ||
            matchers.some((v) => v.trim().length < 2 || v.length > 200)
          ) {
            return yield* new WorkspaceValidationError({
              message: "An email scope must contain bounded, non-empty matchers",
            });
          }
          yield* workspaceRepositoryEffect("enable workspace email scope", () =>
            upsertWorkspaceEmailScope(
              action.workspaceId,
              action.subjectId,
              scope as WorkspaceEmailScopePayload,
            ),
          );
          return yield* finishEffect(actionId, "approved", "Email scope enabled");
        }
        const event = yield* decodePayload(
          actionId,
          action.payload,
          CalendarEventPayloadSchema,
        );
        if (!event.title.trim() || !event.startDate.trim())
          return yield* new WorkspaceValidationError({
            message: "A calendar event requires a title and start date",
          });
        const session = yield* discoverCaldavSessionEffect(logger);
        const result = yield* createCalendarEventEffect(
          session,
          {
            action: "create",
            eventId: undefined,
            duration: undefined,
            recurrence: undefined,
            ...(event as WorkspaceCalendarEventPayload),
          },
          logger,
          `workspace-${action.actionId}@omni-notify`,
        );
        if (result.status === "error" && result.code !== 412)
          return yield* new WorkspaceActionError({ actionId, message: result.message });
        return yield* finishEffect(
          actionId,
          "approved",
          result.status === "success"
            ? `Calendar event created (${result.eventUid})`
            : "Calendar event was already created",
        );
      }).pipe(
        Effect.tapError((error) =>
          workspaceRepositoryEffect("record workspace action failure", () =>
            setWorkspaceActionResult(actionId, "failed", error.message),
          ).pipe(
            Effect.catch((persistenceError) =>
              logger.error(
                `Failed to record workspace action ${actionId} failure`,
                persistenceError,
              ),
            ),
          ),
        ),
      ),
    () => Effect.sync(() => resolvingActions.delete(actionId)),
  );
}

export function rejectWorkspaceActionEffect(actionId: string) {
  const acquire = Effect.gen(function* () {
    if (resolvingActions.has(actionId)) {
      return yield* new WorkspaceActionError({
        actionId,
        message: "Workspace action is already being resolved",
      });
    }
    const action = yield* workspaceRepositoryEffect("read workspace action", () =>
      getWorkspaceAction(actionId),
    );
    if (!action) {
      return yield* new WorkspaceActionError({
        actionId,
        message: "Workspace action not found",
      });
    }
    if (action.status !== "pending") {
      return yield* new WorkspaceActionError({
        actionId,
        message: `Workspace action is already ${action.status}`,
      });
    }
    resolvingActions.add(actionId);
  });
  return Effect.acquireUseRelease(
    acquire,
    () => finishEffect(actionId, "rejected", "Rejected by user"),
    () => Effect.sync(() => resolvingActions.delete(actionId)),
  );
}
