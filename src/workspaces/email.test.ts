import { describe, expect, it } from "vitest";
import type { FetchedEmail } from "../email/types.js";
import { matchesWorkspaceEmail } from "./email.js";
import type { WorkspaceEmailScopeData } from "./persistence.js";

const email: FetchedEmail = {
  id: "mail-1",
  subject: "Your Framework Laptop order shipped",
  from: "Framework <orders@frame.work>",
  textBody: "Track order ABC123 and review your invoice.",
  links: [],
  receivedAt: "2026-08-17T12:00:00Z",
  attachments: [],
};

function scope(overrides: Partial<WorkspaceEmailScopeData>): WorkspaceEmailScopeData {
  return {
    workspaceId: "purchase-research",
    subjectId: "laptop",
    senders: [],
    domains: [],
    subjectKeywords: [],
    bodyKeywords: [],
    updatedAt: 0,
    ...overrides,
  };
}

describe("matchesWorkspaceEmail", () => {
  it("matches exact normalized sender addresses", () => {
    expect(
      matchesWorkspaceEmail(email, scope({ senders: ["orders@frame.work"] })),
    ).toBe(true);
    expect(matchesWorkspaceEmail(email, scope({ senders: ["other@frame.work"] }))).toBe(
      false,
    );
  });

  it("matches domains and strips an optional leading @", () => {
    expect(matchesWorkspaceEmail(email, scope({ domains: ["@frame.work"] }))).toBe(
      true,
    );
    expect(matchesWorkspaceEmail(email, scope({ domains: ["work.example"] }))).toBe(
      false,
    );
  });

  it("matches subject and body keywords case-insensitively", () => {
    expect(
      matchesWorkspaceEmail(email, scope({ subjectKeywords: ["LAPTOP ORDER"] })),
    ).toBe(true);
    expect(matchesWorkspaceEmail(email, scope({ bodyKeywords: ["abc123"] }))).toBe(
      true,
    );
  });

  it("does not ingest an email when the approved scope has no match", () => {
    expect(matchesWorkspaceEmail(email, scope({ subjectKeywords: ["camera"] }))).toBe(
      false,
    );
  });
});
