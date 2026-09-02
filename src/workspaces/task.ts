import type { NamedLogger } from "@micthiesen/mitools/logging";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Effect, Schema } from "effect";
import { WorkspaceValidationError } from "./errors.js";
import { runWorkspaceEffect } from "./engine.js";
import { listWorkspaceSubjects } from "./persistence.js";
import { workspaceRepositoryEffect } from "./repository.js";
import type { WorkspaceDefinition, WorkspaceRunResult } from "./types.js";
import type { TaskServices } from "../task-runs/registry.js";

const manualInputSchema = Schema.Struct({
  message: Schema.String,
  subjectId: Schema.optional(Schema.String),
  trigger: Schema.optional(Schema.Literals(["message", "email"])),
});

export class WorkspaceTask implements ScheduledTask<unknown, TaskServices> {
  public readonly name: string;
  public readonly displayName: string;
  public readonly schedule: string;
  private readonly logger: NamedLogger;
  private lastSummary?: string;

  public constructor(
    private readonly definition: WorkspaceDefinition,
    parentLogger: NamedLogger,
  ) {
    this.name = definition.taskName;
    this.displayName = definition.title;
    this.schedule = definition.schedule;
    this.logger = parentLogger.extend(`${definition.taskName}Task`);
  }

  public readonly run = Effect.gen({ self: this }, function* () {
    if (this.definition.scheduledRuns === false) {
      this.lastSummary = "On-demand workspace; scheduled refresh skipped";
      return;
    }
    const subjects = (yield* workspaceRepositoryEffect(
      "list active workspace subjects",
      () => listWorkspaceSubjects(this.definition.id),
    )).filter((subject) => subject.status === "active");
    if (subjects.length === 0) {
      this.lastSummary = "No active subjects to research";
      return;
    }
    let updated = 0;
    let actions = 0;
    const failures: string[] = [];
    const results = yield* Effect.forEach(subjects, (subject) =>
      Effect.result(
        runWorkspaceEffect(
          this.definition,
          { trigger: "scheduled", subjectId: subject.subjectId },
          this.logger,
        ),
      ).pipe(Effect.map((result) => ({ subject, result }))),
    );
    for (const { subject, result } of results) {
      if (result._tag === "Success") {
        const value = result.success;
        updated += value.updatedSubjects;
        actions += value.createdActions;
      } else {
        const message = result.failure.message;
        failures.push(`${subject.title}: ${message}`);
        yield* this.logger.warn(
          `Workspace research failed for ${subject.title}`,
          message,
        );
      }
    }
    this.lastSummary = `Updated ${updated} subject(s), proposed ${actions} action(s), ${failures.length} failed`;
    if (failures.length > 0) {
      return yield* new WorkspaceValidationError({
        message: `${this.lastSummary}: ${failures.join("; ")}`,
      });
    }
  });

  public runManual(input: unknown) {
    return Effect.gen({ self: this }, function* () {
      const parsed = yield* Schema.decodeUnknownEffect(manualInputSchema)(input).pipe(
        Effect.mapError(
          (cause) =>
            new WorkspaceValidationError({
              message: "Invalid workspace manual input",
              cause,
            }),
        ),
      );
      const message = parsed.message.trim();
      if (
        !message ||
        message.length > 20_000 ||
        (parsed.subjectId !== undefined && !parsed.subjectId)
      ) {
        return yield* new WorkspaceValidationError({
          message: "Workspace manual input is empty or too long",
        });
      }
      const result: WorkspaceRunResult = yield* runWorkspaceEffect(
        this.definition,
        {
          trigger: parsed.trigger ?? "message",
          message,
          subjectId: parsed.subjectId,
        },
        this.logger,
      );
      this.lastSummary = result.summary;
    });
  }

  public getLastRunSummary(): string | undefined {
    return this.lastSummary;
  }
}
