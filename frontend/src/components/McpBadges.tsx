import type { McpCallStatus } from "../api";

const STATUS_LABELS: Record<McpCallStatus, string> = {
  running: "Running",
  ok: "OK",
  error: "Error",
  interrupted: "Interrupted",
};

const POLICY_LABELS: Record<string, string> = {
  allow: "Allow",
  require_approval: "Approval",
  block: "Block",
};

export function PolicyBadge({ policy }: { policy: string }) {
  return (
    <span
      className={`mcp-policy mcp-policy-${policy}`}
      title={`Recommended policy: ${policy.replace(/_/g, " ")}`}
    >
      {POLICY_LABELS[policy] ?? policy}
    </span>
  );
}

export function CallStatusPill({ status }: { status: McpCallStatus }) {
  return (
    <span className={`mcp-status mcp-status-${status}`}>
      <span className="mcp-status-dot" aria-hidden="true" />
      {STATUS_LABELS[status]}
    </span>
  );
}
