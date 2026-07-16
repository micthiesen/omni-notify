import type { TaskRunStatus, TaskTrigger } from "../api";

const TRIGGER_LABELS: Record<TaskTrigger, string> = {
  schedule: "schedule",
  manual: "manual",
  startup: "startup",
  catchup: "catch-up",
};

export function TriggerBadge({ trigger }: { trigger: TaskTrigger }) {
  return (
    <span className={`trigger-badge trigger-${trigger}`}>
      {TRIGGER_LABELS[trigger]}
    </span>
  );
}

export function StatusDot({ status }: { status: TaskRunStatus }) {
  return <span className={`status-dot status-${status}`} />;
}
