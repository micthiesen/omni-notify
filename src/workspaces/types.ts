export type WorkspaceSubjectStatus = "active" | "paused" | "completed" | "archived";

export type WorkspaceArtifactKind =
  | "markdown"
  | "structured"
  | "evidence-ledger"
  | "timeline"
  | "collection";

export interface WorkspaceArtifactDefinition {
  key: string;
  title: string;
  kind: WorkspaceArtifactKind;
  instructions: string;
}

export interface WorkspaceDefinition {
  id: string;
  title: string;
  description: string;
  subjectLabel: string;
  subjectLabelPlural: string;
  taskName: string;
  schedule: string;
  /** False for workspaces that only progress in response to user input. */
  scheduledRuns?: boolean;
  inputPlaceholder?: string;
  followUpPlaceholder?: string;
  instructions: string;
  artifacts: WorkspaceArtifactDefinition[];
}

export interface WorkspaceManualInput {
  message: string;
  subjectId?: string;
  trigger?: "message" | "email";
}

export interface WorkspaceRunRequest {
  trigger: "scheduled" | "message" | "email";
  message?: string;
  subjectId?: string;
}

export interface WorkspaceRunResult {
  summary: string;
  updatedSubjects: number;
  createdActions: number;
}

export type WorkspaceActionType = "email_scope" | "calendar_event";
export type WorkspaceActionStatus = "pending" | "approved" | "rejected" | "failed";

export interface WorkspaceEmailScopePayload {
  senders: string[];
  domains: string[];
  subjectKeywords: string[];
  bodyKeywords: string[];
}

export interface WorkspaceCalendarEventPayload {
  title: string;
  startDate: string;
  endDate?: string;
  startTime?: string;
  endTime?: string;
  location?: string;
  description?: string;
  timeZone?: string;
  allDay: boolean;
  reminderMinutes?: number;
}
