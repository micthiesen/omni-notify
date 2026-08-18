import config from "../utils/config.js";
import type { WorkspaceDefinition } from "./types.js";

export const purchaseResearchWorkspace: WorkspaceDefinition = {
  id: "purchase-research",
  title: "Purchase Research",
  description:
    "Researches purchases under consideration, maintains evidence-backed comparisons, and follows decisions through delivery and return deadlines.",
  subjectLabel: "Purchase",
  subjectLabelPlural: "Purchases",
  taskName: "PurchaseResearch",
  schedule: config.WORKSPACE_SCHEDULE,
  instructions: `You maintain purchase dossiers for one person. A dossier begins when the user says they are considering or tracking something and ends when it is completed or archived.

Be practical and skeptical. Preserve the user's actual requirements, separate facts from recommendations, cite current sources, record why candidates were rejected, and call out unanswered questions. Never purchase anything, send messages, broaden email access, or write to the calendar directly. Those effects must be emitted as reviewable proposals.

Scheduled runs should work only on active dossiers with unresolved questions, stale evidence, meaningful market changes, or a time-sensitive deadline. Do not manufacture updates merely to appear busy. Notifications are for material changes only.

When the user explicitly asks to watch email, propose a narrow email scope using exact senders/domains and product-specific keywords. Never propose an empty or catch-all scope. When a confirmed purchase has a return or price-adjustment deadline, propose a calendar reminder with enough lead time to act.`,
  artifacts: [
    {
      key: "brief",
      title: "Brief",
      kind: "markdown",
      instructions:
        "The current objective, budget, timing, must-haves, preferences, constraints, and explicit non-goals.",
    },
    {
      key: "requirements",
      title: "Requirements",
      kind: "structured",
      instructions:
        "A concise Markdown checklist grouped into required, preferred, and unresolved requirements.",
    },
    {
      key: "comparison",
      title: "Comparison",
      kind: "structured",
      instructions:
        "A Markdown comparison table of serious candidates, current price when known, evidence-backed strengths, weaknesses, and status.",
    },
    {
      key: "research",
      title: "Research",
      kind: "evidence-ledger",
      instructions:
        "Dated findings with source links, freshness notes, disagreements between sources, and facts that need verification.",
    },
    {
      key: "questions",
      title: "Open Questions",
      kind: "collection",
      instructions:
        "The smallest useful list of questions whose answers could change the decision.",
    },
    {
      key: "decision",
      title: "Decision",
      kind: "timeline",
      instructions:
        "Decision history, rejected candidates and why, final purchase details, delivery state, and return or price-adjustment deadlines.",
    },
  ],
};

export const workspaceDefinitions: WorkspaceDefinition[] = [purchaseResearchWorkspace];

export function getWorkspaceDefinition(id: string): WorkspaceDefinition | undefined {
  return workspaceDefinitions.find((workspace) => workspace.id === id);
}
