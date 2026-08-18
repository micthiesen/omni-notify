import type { Logger } from "@micthiesen/mitools/logging";
import { z } from "zod";
import {
  createCalendarEvent,
  discoverCaldavSession,
} from "../calendar-events/caldav/index.js";
import type { WorkspaceActionData } from "./persistence.js";
import {
  getWorkspaceAction,
  setWorkspaceActionResult,
  upsertWorkspaceEmailScope,
} from "./persistence.js";
import type {
  WorkspaceCalendarEventPayload,
  WorkspaceEmailScopePayload,
} from "./types.js";

const emailScopeSchema: z.ZodType<WorkspaceEmailScopePayload> = z
  .object({
    senders: z.array(z.string().trim().min(2).max(200)).max(20),
    domains: z.array(z.string().trim().min(2).max(200)).max(20),
    subjectKeywords: z.array(z.string().trim().min(2).max(200)).max(20),
    bodyKeywords: z.array(z.string().trim().min(2).max(200)).max(20),
  })
  .refine(
    (value) =>
      value.senders.length +
        value.domains.length +
        value.subjectKeywords.length +
        value.bodyKeywords.length >
      0,
    "An email scope must contain at least one matcher",
  );

const calendarEventSchema: z.ZodType<WorkspaceCalendarEventPayload> = z.object({
  title: z.string().min(1),
  startDate: z.string().min(1),
  endDate: z.string().optional(),
  startTime: z.string().optional(),
  endTime: z.string().optional(),
  location: z.string().optional(),
  description: z.string().optional(),
  timeZone: z.string().optional(),
  allDay: z.boolean(),
  reminderMinutes: z.number().optional(),
});

const resolvingActions = new Set<string>();

export async function approveWorkspaceAction(
  actionId: string,
  logger: Logger,
): Promise<WorkspaceActionData> {
  if (resolvingActions.has(actionId))
    throw new Error("Workspace action is already being resolved");
  const action = getApprovableAction(actionId);
  resolvingActions.add(actionId);
  try {
    if (action.type === "email_scope") {
      const scope = emailScopeSchema.parse(JSON.parse(action.payload));
      upsertWorkspaceEmailScope(action.workspaceId, action.subjectId, scope);
      return finish(actionId, "approved", "Email scope enabled");
    }

    const event = calendarEventSchema.parse(JSON.parse(action.payload));
    const session = await discoverCaldavSession(logger);
    const result = await createCalendarEvent(
      session,
      {
        action: "create",
        eventId: undefined,
        duration: undefined,
        recurrence: undefined,
        ...event,
      },
      logger,
      `workspace-${action.actionId}@omni-notify`,
    );
    if (result.status === "error" && result.code !== 412) {
      throw new Error(result.message);
    }
    return finish(
      actionId,
      "approved",
      result.status === "success"
        ? `Calendar event created (${result.eventUid})`
        : "Calendar event was already created",
    );
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setWorkspaceActionResult(actionId, "failed", message);
    throw error;
  } finally {
    resolvingActions.delete(actionId);
  }
}

export function rejectWorkspaceAction(actionId: string): WorkspaceActionData {
  if (resolvingActions.has(actionId)) {
    throw new Error("Workspace action is already being resolved");
  }
  getPendingAction(actionId);
  return finish(actionId, "rejected", "Rejected by user");
}

function getApprovableAction(actionId: string): WorkspaceActionData {
  const action = getWorkspaceAction(actionId);
  if (!action) throw new Error("Workspace action not found");
  if (action.status !== "pending" && action.status !== "failed") {
    throw new Error(`Workspace action is already ${action.status}`);
  }
  return action;
}

function getPendingAction(actionId: string): WorkspaceActionData {
  const action = getWorkspaceAction(actionId);
  if (!action) throw new Error("Workspace action not found");
  if (action.status !== "pending") {
    throw new Error(`Workspace action is already ${action.status}`);
  }
  return action;
}

function finish(
  actionId: string,
  status: "approved" | "rejected",
  result: string,
): WorkspaceActionData {
  const action = setWorkspaceActionResult(actionId, status, result);
  if (!action) throw new Error("Workspace action disappeared while resolving it");
  return action;
}
