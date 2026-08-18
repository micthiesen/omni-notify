---
name: omni-workspace
description: Create, update, inspect, or manage ongoing projects in Omni workspaces through the local Omni API. Use for purchase research, Facebook Marketplace selling help, listing drafts, pricing, photos, offers, scoped email findings, or any request that refers to an Omni workspace. Also invoke proactively when a request clearly belongs to an existing workspace subject.
---

# Omni Workspace

Operate the deployed workspace through `http://omni.boris/api`. Treat the workspace as the durable source of truth. Do not edit workspace rows directly.

## Workflow

1. Read `GET /api/workspaces`. Match the request to an existing workspace and subject when possible.
   - Use Purchase Research for things the user may buy, including requirements, candidates, and ongoing price research.
   - Use Marketplace Selling for things the user wants to sell, including listing fields, pricing, photos, offers, and handoff planning.
2. For an existing subject, read `GET /api/workspaces/{workspaceId}/subjects/{subjectId}` before acting.
3. Send the user's instruction to `POST /api/workspaces/{workspaceId}/messages` as JSON:
   - Existing subject: `{"message":"...","subjectId":"..."}`
   - New subject: `{"message":"..."}`
4. The response is asynchronous and returns `runId`. Poll `GET /api/task-runs/{runId}/logs` and inspect its `run.status` until it is no longer running, then read the workspace or subject again and summarize the resulting state.
5. Give the user the exact UI link: `http://omni.boris/workspaces/{workspaceId}/{subjectId}`.

Use `curl --fail-with-body --silent --show-error`. JSON-encode user text safely with an existing JSON-capable CLI or a task-specific temporary file, never string interpolation into shell syntax.

## Guardrails

- Never approve a pending action unless the user explicitly asks to approve that specific action. Research messages are not approval.
- Never publish or edit a Marketplace listing, message a buyer, accept an offer, disclose a private address, or arrange a meetup. Keep the private walk-away price out of public listing copy.
- Email ingestion must remain off until an `email_scope` proposal is manually approved. Reject scopes that lack an explicit sender, domain, subject keyword, or body keyword.
- Prefer extending the shared workspace definition and primitives over adding a subject-specific escape hatch.
- Report API or workflow friction as a workspace papercut when the app can still complete the user's task.
- If Omni is unavailable, inspect health and production logs before changing data or code.
