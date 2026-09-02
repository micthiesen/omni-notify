import type { NamedLogger } from "@micthiesen/mitools/logging";
import type { EmailHandler, FetchedEmail } from "../email/types.js";
import { Effect } from "effect";
import { WorkspaceOperationError } from "./errors.js";
import type { WorkspaceEmailScopeData } from "./persistence.js";
import {
  addWorkspaceSource,
  getWorkspaceSource,
  getWorkspaceSubject,
  listAllWorkspaceEmailScopes,
  markWorkspaceSourcesTriggered,
} from "./persistence.js";
import { workspaceRepositoryEffect } from "./repository.js";
import type { TaskServices } from "../task-runs/registry.js";

export type WorkspaceEmailTrigger = (
  workspaceId: string,
  subjectId: string,
  message: string,
  trigger: "email",
) => Effect.Effect<void, WorkspaceOperationError, TaskServices>;

export class WorkspaceEmailHandler implements EmailHandler<
  WorkspaceOperationError,
  TaskServices
> {
  public readonly name = "Workspaces";

  public constructor(
    private readonly trigger: WorkspaceEmailTrigger,
    private readonly logger: NamedLogger,
  ) {}

  public handleEmailsEffect(emails: FetchedEmail[]) {
    return Effect.gen({ self: this }, function* () {
      const scopes = yield* workspaceRepositoryEffect(
        "list workspace email scopes",
        () => listAllWorkspaceEmailScopes(),
      );
      const matches = new Map<
        string,
        Array<{ email: FetchedEmail; sourceId: string }>
      >();
      for (const email of emails) {
        for (const scope of scopes) {
          if (!matchesWorkspaceEmail(email, scope)) continue;
          const subject = yield* workspaceRepositoryEffect(
            "read workspace email subject",
            () => getWorkspaceSubject(scope.workspaceId, scope.subjectId),
          );
          if (subject?.status !== "active") continue;
          const key = `${scope.workspaceId}:${scope.subjectId}`;
          const sourceId = `email:${scope.workspaceId}:${scope.subjectId}:${email.id}`;
          const existing = yield* workspaceRepositoryEffect(
            "read workspace email source",
            () => getWorkspaceSource(sourceId),
          );
          if (existing?.triggeredAt) continue;
          const prior = matches.get(key) ?? [];
          prior.push({ email, sourceId });
          matches.set(key, prior);
          if (!existing)
            yield* workspaceRepositoryEffect("persist workspace email source", () =>
              addWorkspaceSource({
                sourceId,
                workspaceId: scope.workspaceId,
                subjectId: scope.subjectId,
                kind: "email",
                title: email.subject || `(Email from ${email.from})`,
                excerpt: email.textBody.slice(0, 4_000),
                emailId: email.id,
              }),
            );
        }
      }
      for (const [key, matched] of matches) {
        const separator = key.indexOf(":");
        const workspaceId = key.slice(0, separator);
        const subjectId = key.slice(separator + 1);
        yield* this.logger.info(
          `Ingested ${matched.length} scoped email(s) for ${workspaceId}/${subjectId}`,
        );
        yield* this.trigger(
          workspaceId,
          subjectId,
          `Review ${matched.length} newly ingested scoped email(s): ${matched.map(({ email }) => email.subject).join("; ")}`,
          "email",
        );
        yield* workspaceRepositoryEffect("mark workspace email sources triggered", () =>
          markWorkspaceSourcesTriggered(matched.map(({ sourceId }) => sourceId)),
        );
      }
    });
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
