import { encodeDoc } from "@micthiesen/mitools/docstore";
// Rows exactly as the TS engine, approval and outbox code write them.
const rows = {
  "workspace-action": {
    workspaceId: "purchase-research", subjectId: "camera", actionId: "a1", type: "calendar_event",
    status: "pending", title: "Return", description: "Return window",
    payload: JSON.stringify({ title: "Return", startDate: "2026-09-01", allDay: true, reminderMinutes: 1440 }),
    createdAt: 1788197422548, runId: "PurchaseResearch:x",
  },
  "workspace-action-resolved": {
    workspaceId: "purchase-research", subjectId: "camera", actionId: "a2", type: "email_scope",
    status: "approved", title: "Watch", description: "d", payload: "{}", createdAt: 1, runId: undefined,
    result: "Email scope enabled", resolvedAt: 2,
  },
  "workspace-email-scope": {
    workspaceId: "purchase-research", subjectId: "camera", senders: ["a@b.c"], domains: [],
    subjectKeywords: ["lens"], bodyKeywords: [], updatedAt: 3,
  },
  "workspace-papercut": {
    workspaceId: "purchase-research", subjectId: undefined, runId: "r", category: "ui-gap",
    title: "No upload", detail: "d", relatedTool: undefined, papercutId: "p1",
    fingerprint: "purchase-research:ui-gap::no upload", occurrences: 2, firstSeenAt: 4, lastSeenAt: 5,
    status: "open",
  },
  "workspace-notification": {
    notificationId: "action:a1", workspaceId: "purchase-research", subjectId: "camera",
    title: "Approval Needed: Return", message: "Return window", url: "http://omni.boris/x",
    urlTitle: "Review Action", status: "sent", attempts: 1, createdAt: 6, nextAttemptAt: 6,
    lastError: undefined, sentAt: 7,
  },
  "workspace-source-email": {
    sourceId: "email:purchase-research:camera:<m@x>", workspaceId: "purchase-research",
    subjectId: "camera", kind: "email", title: "Price drop", excerpt: "Now cheaper",
    emailId: "<m@x>", createdAt: 8, triggeredAt: 9,
  },
  "briefing-history-null-cost": {
    briefingName: "News",
    notifications: [{ title: "T", message: "M", url: "https://x.y", timestamp: 10, runId: "News:r", costCents: null }],
  },
};
const out = {};
for (const [name, row] of Object.entries(rows)) out[name] = Buffer.from(encodeDoc(row)).toString("hex");
console.log(JSON.stringify(out, null, 2));
