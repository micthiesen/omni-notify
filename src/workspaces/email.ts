import type { Logger } from "@micthiesen/mitools/logging";
import type { EmailHandler, FetchedEmail } from "../email/types.js";
import type { WorkspaceEmailScopeData } from "./persistence.js";
import {
  addWorkspaceSource,
  getWorkspaceSource,
  getWorkspaceSubject,
  listAllWorkspaceEmailScopes,
} from "./persistence.js";

export type WorkspaceEmailTrigger = (
  workspaceId: string,
  subjectId: string,
  message: string,
  trigger: "email",
) => void;

export class WorkspaceEmailHandler implements EmailHandler {
  public readonly name = "Workspaces";

  public constructor(
    private readonly trigger: WorkspaceEmailTrigger,
    private readonly logger: Logger,
  ) {}

  public async handleEmails(emails: FetchedEmail[]): Promise<void> {
    const scopes = listAllWorkspaceEmailScopes();
    const matches = new Map<string, FetchedEmail[]>();
    for (const email of emails) {
      for (const scope of scopes) {
        if (!matchesWorkspaceEmail(email, scope)) continue;
        const subject = getWorkspaceSubject(scope.workspaceId, scope.subjectId);
        if (subject?.status !== "active") continue;
        const key = `${scope.workspaceId}:${scope.subjectId}`;
        const sourceId = `email:${scope.workspaceId}:${scope.subjectId}:${email.id}`;
        if (getWorkspaceSource(sourceId)) continue;
        const prior = matches.get(key) ?? [];
        prior.push(email);
        matches.set(key, prior);
        addWorkspaceSource({
          sourceId,
          workspaceId: scope.workspaceId,
          subjectId: scope.subjectId,
          kind: "email",
          title: email.subject || `(Email from ${email.from})`,
          excerpt: email.textBody.slice(0, 4_000),
          emailId: email.id,
        });
      }
    }
    for (const [key, matched] of matches) {
      const separator = key.indexOf(":");
      const workspaceId = key.slice(0, separator);
      const subjectId = key.slice(separator + 1);
      this.logger.info(
        `Ingested ${matched.length} scoped email(s) for ${workspaceId}/${subjectId}`,
      );
      this.trigger(
        workspaceId,
        subjectId,
        `Review ${matched.length} newly ingested scoped email(s): ${matched.map((email) => email.subject).join("; ")}`,
        "email",
      );
    }
  }
}

export function matchesWorkspaceEmail(
  email: FetchedEmail,
  scope: WorkspaceEmailScopeData,
): boolean {
  const from = email.from.trim().toLowerCase();
  const address = extractAddress(from);
  const domain = address.split("@")[1] ?? "";
  const subject = email.subject.toLowerCase();
  const body = email.textBody.toLowerCase();
  return (
    scope.senders.some((value) => address === value.trim().toLowerCase()) ||
    scope.domains.some(
      (value) => domain === value.trim().toLowerCase().replace(/^@/, ""),
    ) ||
    scope.subjectKeywords.some((value) => subject.includes(value.toLowerCase())) ||
    scope.bodyKeywords.some((value) => body.includes(value.toLowerCase()))
  );
}

function extractAddress(from: string): string {
  const match = from.match(/<([^>]+)>/);
  return (match?.[1] ?? from).trim();
}
