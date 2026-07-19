import type { EmailActivityOutcome, EmailPipeline } from "../api";

export const PIPELINE_LABELS: Record<EmailPipeline, string> = {
  ParcelTracker: "Parcels",
  CalendarEvents: "Calendar",
};

export const OUTCOME_LABELS: Record<EmailActivityOutcome, string> = {
  filtered: "Filtered",
  skipped: "Skipped",
  no_matches: "No Matches",
  processed: "Processed",
  partial: "Partial",
  failed: "Failed",
  error: "Error",
};
