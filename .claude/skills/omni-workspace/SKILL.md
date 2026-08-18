---
name: omni-workspace
description: Create, update, inspect, or manage ongoing research dossiers in Omni workspaces through the local Omni API. Use when the user asks to research a purchase over time, add or change purchase requirements or candidates, check an ongoing dossier, act on scoped email findings, or otherwise refers to an Omni workspace. Also invoke proactively when a request clearly belongs to an existing ongoing research dossier.
---

# Omni Workspace

Operate the deployed workspace through `http://omni.boris/api`. Treat the workspace as the durable source of truth. Do not edit workspace rows directly.

## Workflow

1. Read `GET /api/workspaces`. Match the request to an existing workspace and subject when possible.
2. For an existing subject, read `GET /api/workspaces/{workspaceId}/subjects/{subjectId}` before acting.
3. Send the user's instruction to `POST /api/workspaces/{workspaceId}/messages` as JSON:
   - Existing subject: `{"message":"...","subjectId":"..."}`
   - New subject: `{"message":"..."}`
4. The response is asynchronous and returns `runId`. Poll `GET /api/task-runs/{runId}/logs` and inspect its `run.status` until it is no longer running, then read the workspace or subject again and summarize the resulting state.
5. Give the user the exact UI link: `http://omni.boris/workspaces/{workspaceId}/{subjectId}`.

Use `curl --fail-with-body --silent --show-error`. JSON-encode user text safely with an existing JSON-capable CLI or a task-specific temporary file, never string interpolation into shell syntax.

## Guardrails

- Never approve a pending action unless the user explicitly asks to approve that specific action. Research messages are not approval.
- Email ingestion must remain off until an `email_scope` proposal is manually approved. Reject scopes that lack an explicit sender, domain, subject keyword, or body keyword.
- Prefer extending the shared workspace definition and primitives over adding a subject-specific escape hatch.
- Report API or workflow friction as a workspace papercut when the app can still complete the user's task.
- If Omni is unavailable, inspect health and production logs before changing data or code.
